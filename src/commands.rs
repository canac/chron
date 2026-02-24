use crate::chron_service::ChronService;
use crate::chronfile::Chronfile;
use crate::chronfile::env::Env;
use crate::cli::{KillArgs, LogsArgs, RunArgs, RunsArgs, StatusArgs, TriggerArgs};
use crate::database::{
    ClientDatabase, HostDatabase, HostServer, JobStatus, RunStatus, TerminateResult, TriggerResult,
};
use crate::format;
use crate::http;
use anyhow::{Context, Result, bail};
use chrono::Local;
use cli_tables::Table;
use log::{LevelFilter, debug, error, info};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use std::io::{BufWriter, IsTerminal, Write, stdin};
use std::path::Path;
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::oneshot::channel;

/// Implementation for the `run` CLI command
pub async fn run(chron_dir: &Path, args: RunArgs) -> Result<()> {
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

    let env = Env::from_host()?;
    let chronfile = Chronfile::load(&chronfile_path, &env).await?;

    let host_server = HostServer::new(chron_dir).await?;

    let db = Arc::new(HostDatabase::open(chron_dir).await?);
    let mut chron = ChronService::new(chron_dir, Arc::clone(&db));
    chron.start(chronfile).await?;
    let chron_lock = Arc::new(RwLock::new(chron));

    host_server.start(Arc::clone(&chron_lock)).await;

    let watcher_chron = Arc::clone(&chron_lock);
    let watch_path = chronfile_path.clone();
    let handle = Handle::current();
    let mut debouncer = new_debouncer(Duration::from_secs(1), move |res: DebounceEventResult| {
        if res.is_err() {
            return;
        }

        handle.block_on(async {
            match Chronfile::load(&watch_path, &env).await {
                Ok(chronfile) => {
                    debug!("Reloading chronfile {}", watch_path.to_string_lossy());
                    if let Err(err) = watcher_chron.write().await.start(chronfile).await {
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
    let mut tx = Some(tx);
    let second_signal = AtomicBool::new(false);
    ctrlc::set_handler(move || {
        if second_signal.swap(true, Ordering::Relaxed) {
            info!("Shutting down forcefully");
            exit(1);
        }

        info!("Shutting down gracefully...");
        if stdin().is_terminal() {
            info!("To shut down immediately, press Ctrl-C again");
        }
        if let Some(tx) = tx.take() {
            let _ = tx.send(());
        }
    })?;

    if let Some(port) = port {
        // Start the HTTP server
        let client_db = Arc::new(ClientDatabase::open(chron_dir).await?);
        let server = http::create_server(&chron_lock, &client_db, port).await?;
        let handle = server.handle();
        tokio::spawn(async move {
            let _ = rx.await;
            info!("Stopping HTTP server");
            handle.stop(true).await;
        });
        server.await?;
    } else {
        let _ = rx.await;
    }
    drop(debouncer);

    host_server.close().await?;

    let Some(chron) = Arc::into_inner(chron_lock) else {
        bail!("Failed to shutdown because the chron service is still in use");
    };
    chron.into_inner().stop().await?;

    if Arc::into_inner(db).is_none() {
        bail!("Failed to shutdown because the database is still in use")
    }

    Ok(())
}

/// Implementation for the `jobs` CLI command
pub async fn jobs(db: ClientDatabase) -> Result<()> {
    let jobs = db.get_active_jobs().await?;
    if jobs.is_empty() {
        println!("No jobs are running");
        return Ok(());
    }
    let mut table = Table::new();
    table.push_row(&vec!["name", "command", "status"])?;
    for job in jobs {
        let status = match job.status {
            JobStatus::Running { .. } => "running".to_owned(),
            JobStatus::Waiting { next_run } => {
                let next_run = next_run.with_timezone(&Local);
                format!("next run {}", format::relative_date(&next_run))
            }
            JobStatus::Completed => "next run never".to_owned(),
        };
        table.push_row_string(&vec![job.name, job.config.command, status])?;
    }
    println!("{}", table.to_string());

    Ok(())
}

/// Implementation for the `status` CLI command
pub async fn status(db: ClientDatabase, args: StatusArgs) -> Result<()> {
    let StatusArgs { job } = args;
    let Some(job) = db.get_active_job(job.clone()).await? else {
        bail!("Job {job} is not running");
    };

    println!("command: {}", job.config.command);
    if let Some(working_dir) = job.config.working_dir {
        println!("working directory: {}", working_dir.display());
    }
    if let Some(schedule) = job.config.schedule {
        println!("schedule: {schedule}");
    }

    let status = match job.status {
        JobStatus::Running { pid } => format!("running (pid {pid})"),
        JobStatus::Completed => "not running".to_owned(),
        JobStatus::Waiting { next_run } => format!("not running (next run at {next_run})"),
    };
    println!("status: {status}");

    Ok(())
}

/// Implementation for the `runs` CLI command
pub async fn runs(db: ClientDatabase, args: RunsArgs) -> Result<()> {
    let name = args.job;
    if db.get_active_job(name.clone()).await?.is_none() {
        bail!("Job {name} is not running")
    }
    let runs = db.get_last_runs(name.clone(), 10).await?;
    if runs.is_empty() {
        bail!("No runs found for job {name}");
    }

    let mut table = Table::new();
    table.push_row(&vec!["time", "execution time", "status"])?;
    for run in runs {
        let status = match run.status()? {
            RunStatus::Running { pid } => format!("running (pid {pid})"),
            RunStatus::Completed { status_code, .. } => status_code.to_string(),
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
pub async fn logs(db: ClientDatabase, args: LogsArgs) -> Result<()> {
    let LogsArgs {
        job: name,
        lines,
        follow,
    } = args;

    let job = db.get_active_job(name.clone()).await?;
    let run_id = db
        .get_last_runs(name.clone(), 1)
        .await?
        .first()
        .map(|run| run.id);
    let (Some(job), Some(run_id)) = (job, run_id) else {
        bail!("Job {name} is not running")
    };

    let file = OpenOptions::new()
        .read(true)
        .open(job.config.log_dir.join(format!("{run_id}.log")))
        .await
        .context("Failed to open log file")?;
    let mut reader = BufReader::new(file);

    let mut logs = String::new();
    reader
        .read_to_string(&mut logs)
        .await
        .context("Failed to read log file")?;

    if let Some(lines) = lines {
        let log_lines = logs.lines().collect::<Vec<_>>();
        let start = log_lines.len().saturating_sub(lines);

        let mut writer = BufWriter::new(std::io::stdout().lock());
        for line in &log_lines[start..] {
            writeln!(writer, "{line}")?;
        }
        writer.flush()?;
    } else {
        print!("{logs}");
    }

    if follow {
        loop {
            let status = db.get_run_status(run_id).await?;
            if !matches!(status, Some(RunStatus::Running { .. })) {
                let mut remaining = String::new();
                reader.read_to_string(&mut remaining).await?;
                if !remaining.is_empty() {
                    print!("{remaining}");
                }
                break;
            }

            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read > 0 {
                print!("{line}");
            } else {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

/// Implementation for the `trigger` CLI command
pub async fn trigger(mut db: ClientDatabase, args: TriggerArgs) -> Result<()> {
    let TriggerArgs { job } = args;
    match db.trigger_job(&job).await? {
        TriggerResult::Started => println!("Triggered job {job}"),
        TriggerResult::Running { pid } => bail!("Job {job} is already running (pid {pid})"),
        TriggerResult::NotFound => bail!("Job {job} not found"),
    }
    Ok(())
}

/// Implementation for the `kill` CLI command
pub async fn kill(mut db: ClientDatabase, args: KillArgs) -> Result<()> {
    let KillArgs { job } = args;
    match db.terminate_job(&job).await? {
        TerminateResult::Terminated { pid } => println!("Terminated process {pid}"),
        TerminateResult::NotRunning => bail!("Job {job} is not running"),
        TerminateResult::NotFound => bail!("Job {job} not found"),
    }
    Ok(())
}
