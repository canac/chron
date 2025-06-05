use super::attempt::Attempt;
use super::sleep::sleep_until;
use super::{Job, RetryConfig};
use crate::chron_service::Process;
use crate::database::Database;
use crate::result_ext::ResultExt;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs::{OpenOptions, create_dir_all};
use tokio::process::Command;
use tokio::sync::oneshot::channel;

/// Create a mailbox message indicating that the job failed
async fn write_mailbox_message(name: &str, code: i32) -> Result<()> {
    warn!("{name}: failed with exit code {code}");

    Command::new("mailbox")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "add",
            &format!("chron/error/{name}"),
            &format!("{name} failed with exit code {code}"),
        ])
        .spawn()?
        .wait()
        .await?;
    Ok(())
}

/// Execute the specified command without retries
/// Return its next attempt time, if any
async fn exec_command_once(
    db: &Arc<Database>,
    job: &Arc<Job>,
    retry_config: &RetryConfig,
    attempt: &Attempt<'_>,
) -> Result<Option<DateTime<Utc>>> {
    let name = job.name.clone();
    info!(
        "{name}: running \"{}\" with shell \"{}\"{}",
        job.command,
        job.shell,
        job.working_dir
            .as_ref()
            .map(|dir| format!(" in directory \"{}\"", dir.to_string_lossy()))
            .unwrap_or_default()
    );

    // Record the run in the database
    let run = db
        .insert_run(
            name.clone(),
            attempt.scheduled_time.naive_utc(),
            attempt.attempt,
            retry_config.limit,
        )
        .await?;

    // Open the log file, creating the directory if necessary
    create_dir_all(&job.log_dir)
        .await
        .with_context(|| format!("Failed to create log dir {}", job.log_dir.display()))?;
    let log_path = job.log_dir.join(format!("{}.log", run.id));
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .with_context(|| format!("Failed to open log file {}", log_path.display()))?
        .into_std()
        .await;

    // Run the command
    let mut command = Command::new(&job.shell);
    command
        .args(["-c", &job.command])
        .stdin(Stdio::null())
        .stdout(log_file.try_clone().context("Failed to clone log file")?)
        .stderr(log_file);
    if let Some(working_dir) = &job.working_dir {
        command.current_dir(working_dir);
    }
    let mut process = command
        .spawn()
        .with_context(|| format!("Failed to run command {}", job.command))?;

    // Link the process to the database run
    let pid = process
        .id()
        .ok_or_else(|| anyhow!("Process has already exited"))?;
    db.set_run_pid(name.clone(), pid).await?;

    let (tx_terminate, rx_terminate) = channel();
    let (tx_terminated, rx_terminated) = channel();
    *job.running_process.write().await = process.id().map(|pid| Process {
        pid,
        terminate: Some((tx_terminate, rx_terminated)),
    });

    // Wait for the process to exit
    let (status_code, terminated) = tokio::select! {
        tx_result = rx_terminate => {
            let terminated = tx_result.is_ok();
            if terminated {
                info!("{}: killing running process \"{}\"", job.name, job.command);

                // Ignore InvalidInput errors, which are because the process already terminated
                process.kill().await.filter_err(|err| err.kind() != std::io::ErrorKind::InvalidInput).with_context(|| {
                    format!("Failed to terminate command {}", job.command)
                })?;
            }
            (None, terminated)
        }
        status = process.wait() => {
            (status?.code(), false)
        }
    };

    // Update the run status code in the database
    let next_attempt = attempt.next_attempt(status_code, retry_config);
    *job.next_attempt.write().await = next_attempt;
    let next_scheduled_run = job.next_scheduled_run().await;
    db.complete_run(
        name.clone(),
        status_code,
        next_attempt.or(next_scheduled_run).as_ref(),
    )
    .await?;

    // Wait to clear the process until after saving the run status to the database to avoid a race condition where the
    // process is None because the job terminated, but the most recent run in the database still has a status of None.
    *job.running_process.write().await = None;

    if let Some(code) = status_code {
        if code != 0 {
            warn!("{name}: failed with exit code {code}");

            if write_mailbox_message(&name, code).await.is_err() {
                warn!("Failed to write mailbox message");
            }
        }
    }

    // Notify the caller that post-termination cleanup is complete
    if terminated {
        let _ = tx_terminated.send(());
    }

    Ok(next_attempt)
}

/// Execute the job's command, handling retries
pub async fn exec_command(
    db: &Arc<Database>,
    job: &Arc<Job>,
    retry_config: &RetryConfig,
    scheduled_time: &DateTime<Utc>,
) -> Result<()> {
    let name = job.name.clone();
    let num_attempts = retry_config
        .limit
        .map_or_else(|| String::from("unlimited"), |limit| limit.to_string());
    for attempt in 0.. {
        if attempt > 0 {
            debug!("{name}: retry attempt {attempt} of {num_attempts}");
        }

        let attempt = Attempt {
            scheduled_time,
            attempt,
        };
        let next_attempt = exec_command_once(db, job, retry_config, &attempt).await?;
        match next_attempt {
            // Wait until the next attempt before looping again
            Some(next_attempt) => sleep_until(next_attempt).await,
            // There are no more attempts
            None => break,
        }
    }

    Ok(())
}
