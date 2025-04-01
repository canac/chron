use super::sleep::sleep_until;
use super::{DatabaseMutex, Job, RetryConfig};
use crate::chron_service::Process;
use crate::sync_ext::{MutexExt, RwLockExt};
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use std::fs;
use std::process::{self, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

pub struct Metadata<'t> {
    pub scheduled_time: &'t DateTime<Utc>,
    pub attempt: usize,
    pub max_attempts: Option<usize>,
}

#[derive(Clone, Copy)]
pub enum ExecStatus {
    Success,
    Failure,
    Aborted,
}

// Helper to execute the specified command without retries
fn exec_command_once(
    db: &DatabaseMutex,
    job: &Arc<Job>,
    metadata: &Metadata,
) -> Result<ExecStatus> {
    // Don't run the job at all if it is supposed to be terminated
    if job.terminate_controller.is_terminated() {
        return Ok(ExecStatus::Aborted);
    }

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
    let run = db.lock_unpoisoned().insert_run(
        &name,
        &metadata.scheduled_time.naive_utc(),
        metadata.attempt,
        metadata.max_attempts,
    )?;

    // Open the log file, creating the directory if necessary
    fs::create_dir_all(&job.log_dir)
        .with_context(|| format!("Failed to create log dir {:?}", job.log_dir))?;
    let log_path = job.log_dir.join(format!("{}.log", run.id));
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file {log_path:?}"))?;

    // Run the command
    let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
    let mut command = process::Command::new(&job.shell);
    command
        .args(["-c", &job.command])
        .stdin(process::Stdio::null())
        .stdout(clone_log_file()?)
        .stderr(clone_log_file()?);
    if let Some(working_dir) = &job.working_dir {
        command.current_dir(working_dir);
    }
    let process = command
        .spawn()
        .with_context(|| format!("Failed to run command {}", job.command))?;

    let mut process_guard = job.running_process.write_unpoisoned();
    *process_guard = Some(Process {
        child_process: process,
        run_id: run.id,
    });
    drop(process_guard);

    // Check the status periodically until it exits without holding onto the child process lock
    let status_code = poll_exit_status(job)?;

    // Update the run status code in the database
    db.lock_unpoisoned()
        .set_run_status_code(run.id, status_code)?;

    // Wait to clear the process until after saving the run status to the database to avoid a race condition where the
    // process is None because the job terminated, but the most recent run in the database still has a status of None.
    *job.running_process.write_unpoisoned() = None;

    if let Some(code) = status_code {
        if code != 0 {
            warn!("{name}: failed with exit code {code}");

            if Command::new("mailbox")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args([
                    "add",
                    &format!("chron/error/{name}"),
                    &format!("{name} failed with exit code {code}"),
                ])
                .spawn()
                .is_err()
            {
                warn!("Failed to write mailbox message");
            }
        }
    }

    Ok(match status_code {
        Some(0) => ExecStatus::Success,
        _ => ExecStatus::Failure,
    })
}

// Check the status of a job until it exits without holding onto the child process lock
fn poll_exit_status(job: &Arc<Job>) -> Result<Option<i32>> {
    const MAX_POLL_DELAY: Duration = Duration::from_millis(500);

    // Check the status periodically until it exits without holding onto the child process lock
    let mut poll_interval = Duration::from_millis(1);
    loop {
        let mut process_guard = job.running_process.write_unpoisoned();
        let process = process_guard
            .as_mut()
            .map(|process| &mut process.child_process)
            .ok_or_else(|| anyhow!("No process to poll"))?;

        // Attempt to terminate the process if we got a terminate signal from the terminate controller
        if job.terminate_controller.is_terminated() {
            let result = process.kill();

            // If the result was an InvalidInput error, it is because the process already
            // terminated, so ignore that type of error
            match result {
                Ok(()) => return Ok(None),
                Err(ref err) => {
                    // Propagate other errors
                    if !matches!(&err.kind(), std::io::ErrorKind::InvalidInput) {
                        result.with_context(|| {
                            format!("Failed to terminate command {}", job.command)
                        })?;
                    }
                }
            }
        }

        // Try to read the process' exit status without blocking
        let exit_status = process
            .try_wait()
            .with_context(|| format!("Failed to get command status for command {}", job.command))?;
        drop(process_guard);
        if let Some(status) = exit_status {
            return Ok(status.code());
        }

        // Wait briefly with capped exponential backoff before looping again
        poll_interval = std::cmp::min(poll_interval * 2, MAX_POLL_DELAY);
        if job.terminate_controller.wait_blocking(poll_interval)? {
            return Ok(None);
        }
    }
}

// Execute the job's command, handling retries
// Return a boolean indicating whether the command completed
pub fn exec_command(
    db: &DatabaseMutex,
    job: &Arc<Job>,
    retry_config: &RetryConfig,
    scheduled_time: &DateTime<Utc>,
) -> Result<bool> {
    let name = job.name.clone();
    let num_attempts = retry_config
        .limit
        .map_or_else(|| String::from("unlimited"), |limit| limit.to_string());
    for attempt in 0.. {
        // Stop executing if a terminate was requested
        if job.terminate_controller.is_terminated() {
            break;
        }

        if attempt > 0 {
            debug!("{name}: retry attempt {attempt} of {num_attempts}");
        }

        let metadata = Metadata {
            scheduled_time,
            attempt,
            max_attempts: retry_config.limit,
        };
        let status = exec_command_once(db, job, &metadata)?;
        if !retry_config.should_retry(status, attempt) {
            // We are done retrying so clear the next attempt
            job.next_attempt.write_unpoisoned().take();
            break;
        }

        // Stop executing if a terminate was requested
        if job.terminate_controller.is_terminated() {
            break;
        }

        // Record the timestamp of the next attempt and wait until then to re-run the job
        let next_attempt = Utc::now()
            + retry_config
                .delay
                .and_then(|delay| chrono::Duration::from_std(delay).ok())
                .unwrap_or_default();
        *job.next_attempt.write_unpoisoned() = Some(next_attempt);

        sleep_until(next_attempt, &job.terminate_controller)?;
    }

    // The command is only considered incomplete if it aborted
    Ok(!job.terminate_controller.is_terminated())
}
