use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use crate::cli::{KillArgs, LogsArgs, RunArgs, RunsArgs, StatusArgs};
use crate::database::{Database, RunStatus};
use crate::format;
use crate::http;
use crate::http::api::JobStatus;
use anyhow::{Context, Result, bail};
use chrono::Local;
use cli_tables::Table;
use log::{LevelFilter, debug, error, info};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use reqwest::header::HeaderValue;
use reqwest::{Client, Response, StatusCode};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, BufWriter, IsTerminal, Read, Write, stdin};
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::oneshot::channel;

/// Return the directory where chron will store application data
pub fn get_data_dir() -> Result<PathBuf> {
    let project_dirs = directories::ProjectDirs::from("com", "canac", "chron")
        .context("Failed to determine application directories")?;
    Ok(project_dirs.data_local_dir().to_owned())
}

/// Implementation for the `run` CLI command
pub async fn run(db: Arc<Database>, args: RunArgs) -> Result<()> {
    let RunArgs {
        port,
        quiet,
        chronfile: chronfile_path,
    } = args;

    simple_logger::SimpleLogger::new()
        .with_module_level("actix_server", LevelFilter::Off)
        .with_module_level("mio", LevelFilter::Off)
        .with_level(if quiet {
            LevelFilter::Warn
        } else {
            LevelFilter::Debug
        })
        .init()?;

    let chronfile = Chronfile::load(&chronfile_path).await?;

    let chron = ChronService::new(&get_data_dir()?, Arc::clone(&db))?;
    let chron_lock = Arc::new(RwLock::new(chron));
    let (server, port) = http::create_server(&chron_lock, &db, port)?;
    // Release any jobs associated with an old chron process using this port. The fact that we bound to the port is
    // proof that the old process is no longer running.
    db.release_port(port).await?;
    chron_lock.write().await.start(chronfile, port).await?;

    let watcher_chron = Arc::clone(&chron_lock);
    let watch_path = chronfile_path.clone();
    let handle = Handle::current();
    let mut debouncer = new_debouncer(Duration::from_secs(1), move |res: DebounceEventResult| {
        if res.is_err() {
            return;
        }

        handle.block_on(async {
            match Chronfile::load(&watch_path).await {
                Ok(chronfile) => {
                    debug!("Reloading chronfile {}", watch_path.to_string_lossy());
                    if let Err(err) = watcher_chron.write().await.start(chronfile, port).await {
                        error!("Failed to start chron\n{err:?}");
                    }
                }
                Err(err) => error!(
                    "Failed to parse chronfile {}\n{err:?}",
                    watch_path.to_string_lossy()
                ),
            }
        });
    })
    .context("Failed to create watcher debouncer")?;
    debouncer
        .watcher()
        .watch(&chronfile_path, RecursiveMode::NonRecursive)
        .context("Failed to start chronfile watcher")?;

    let (tx, rx) = channel();
    let ctrlc_chron = Arc::clone(&chron_lock);
    let mut tx = Some(tx);
    let second_signal = AtomicBool::new(false);
    let handle = Handle::current();
    ctrlc::set_handler(move || {
        if second_signal.swap(true, Ordering::Relaxed) {
            info!("Shutting down forcefully");
            exit(1);
        }

        info!("Shutting down gracefully...");
        if stdin().is_terminal() {
            info!("To shut down immediately, press Ctrl-C again");
        }
        handle.block_on(async {
            ctrlc_chron
                .write()
                .await
                .stop()
                .await
                .expect("Failed to stop chron");
        });
        if let Some(tx) = tx.take() {
            tx.send(()).expect("Failed to send terminate message");
        }
    })?;

    // Start the HTTP server
    let handle = server.handle();
    tokio::select! {
        _ = server => (),
        _ = rx => {
            info!("Stopping HTTP server");
            handle.stop(true).await;
        },
    }

    Ok(())
}

fn validate_response(job: &str, res: &Response) -> Result<()> {
    if res.headers().get("x-powered-by") != Some(&HeaderValue::from_static("chron")) {
        bail!(
            "Server at {} is not a chron server",
            res.url().origin().ascii_serialization()
        );
    }

    if res.status() == StatusCode::NOT_FOUND {
        bail!("Job {job} is not running");
    }

    Ok(())
}

/// Implementation for the `jobs` CLI command
pub async fn jobs(db: Arc<Database>) -> Result<()> {
    let jobs = db.get_active_jobs(ChronService::check_port_active).await?;
    if jobs.is_empty() {
        println!("No jobs are running");
        return Ok(());
    }
    let mut table = Table::new();
    table.push_row(&vec!["name", "command", "status"])?;
    for job in jobs {
        let status = if job.running {
            "running".to_owned()
        } else {
            let next_run = job.next_run.map_or_else(
                || "never".to_owned(),
                |next_run| format::relative_date(&next_run.with_timezone(&Local)),
            );
            format!("next run {next_run}")
        };
        table.push_row_string(&vec![job.name, job.command, status])?;
    }
    println!("{}", table.to_string());

    Ok(())
}

/// Implementation for the `status` CLI command
pub async fn status(db: Arc<Database>, args: StatusArgs) -> Result<()> {
    let StatusArgs { job } = args;
    let port = get_job_port(&db, job.clone()).await?;
    let origin = format!("http://localhost:{port}");
    let res = reqwest::get(format!("{origin}/api/job/{job}/status"))
        .await
        .context("Failed to connect to chron server")?;
    validate_response(&job, &res)?;

    let status: JobStatus = res.json().await?;
    println!("command: {}", status.command);
    println!("shell: {}", status.shell);
    if let Some(schedule) = status.schedule.as_ref() {
        println!("schedule: {schedule}");
    }
    println!("status: {}", status.status);

    Ok(())
}

/// Implementation for the `runs` CLI command
pub async fn runs(db: Arc<Database>, args: RunsArgs) -> Result<()> {
    let RunsArgs { job } = args;
    let port = get_job_port(&db, job.clone()).await?;
    let origin = format!("http://localhost:{port}");
    let res = Client::builder()
        .build()?
        .head(format!("{origin}/api/job/{job}"))
        .send()
        .await
        .context("Failed to connect to chron server")?;
    validate_response(&job, &res)?;

    let runs = db.get_last_runs(job.clone(), 10).await?;

    if runs.is_empty() {
        bail!("No runs found for job {job}");
    }

    let mut table = Table::new();
    table.push_row(&vec!["time", "execution time", "status code"])?;
    for run in runs {
        let status = match run.status() {
            RunStatus::Running => "running".to_owned(),
            RunStatus::Completed { status_code } => status_code.to_string(),
            RunStatus::Terminated => "terminated".to_owned(),
        };
        table.push_row_string(&vec![
            format::relative_date(&run.started_at()),
            run.execution_time()
                .map(|duration| format::duration(&duration))
                .unwrap_or_default(),
            status,
        ])?;
    }
    println!("{}", table.to_string());

    Ok(())
}

/// Implementation for the `logs` CLI command
pub async fn logs(db: Arc<Database>, args: LogsArgs) -> Result<()> {
    let LogsArgs { job, lines, follow } = args;
    let port = get_job_port(&db, job.clone()).await?;
    let origin = format!("http://localhost:{port}");
    let res = reqwest::get(format!("{origin}/api/job/{job}/log_path"))
        .await
        .context("Failed to connect to chron server")?;
    validate_response(&job, &res)?;

    let log_path = res.text().await?;
    let file = OpenOptions::new()
        .read(true)
        .open(&log_path)
        .context("Failed to open log file")?;
    let mut reader = BufReader::new(file);

    let mut logs = String::new();
    reader
        .read_to_string(&mut logs)
        .context("Failed to read log file")?;

    if let Some(lines) = lines {
        let log_lines: Vec<&str> = logs.lines().collect();
        let start = log_lines.len().saturating_sub(lines);

        let mut writer = BufWriter::new(std::io::stdout().lock());
        for line in &log_lines[start..] {
            writeln!(writer, "{line}")?;
        }
        writer.flush()?;
    } else {
        println!("{logs}");
    }

    if follow {
        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read > 0 {
                print!("{line}");
            } else {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    Ok(())
}

/// Implementation for the `kill` CLI command
pub async fn kill(db: Arc<Database>, args: KillArgs) -> Result<()> {
    let KillArgs { job } = args;
    let port = get_job_port(&db, job.clone()).await?;
    let origin = format!("http://localhost:{port}");
    let res = Client::builder()
        .build()?
        .post(format!("{origin}/api/job/{job}/terminate"))
        .send()
        .await
        .context("Failed to connect to chron server")?;
    validate_response(&job, &res)?;

    let pid: i32 = res.json().await?;
    println!("Terminated process {pid}");

    Ok(())
}

/// Get the port of a job, returning an error if it is not running
async fn get_job_port(db: &Arc<Database>, job: String) -> Result<u16> {
    match db.get_job_port(job.clone()).await? {
        Some(port) => Ok(port),
        None => bail!("Job {job} is not running"),
    }
}
