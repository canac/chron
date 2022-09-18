use super::terminate_controller::TerminateController;
use super::RetryConfig;
use crate::chron_service::{ChronServiceLock, JobLock};
use crate::sleep::sleep_duration;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use log::{debug, info, warn};
use std::fs;
use std::io::Write;
use std::process::{self, Command, Stdio};
use std::time::Duration;

pub(crate) enum ExecStatus {
    Success,
    Failure,
    Aborted,
}

// Helper to execute the specified command without retries
fn exec_command_once(
    chron_lock: &ChronServiceLock,
    job_lock: &JobLock,
    terminate_controller: &TerminateController,
) -> Result<ExecStatus> {
    // Don't run the job at all if it is supposed to be terminated
    if terminate_controller.is_terminated() {
        return Ok(ExecStatus::Aborted);
    }

    let start_time = chrono::Local::now();
    let formatted_start_time = start_time.to_rfc3339();

    let mut job = job_lock.write().unwrap();
    let name = job.name.clone();
    info!(
        "{name}: running \"{}\" with shell \"{}\"",
        job.command, job.shell
    );

    // Record the run in the database
    let run = chron_lock
        .read()
        .unwrap()
        .get_db()
        .lock()
        .unwrap()
        .insert_run(&name)?;

    // Open the log file, creating the directory if necessary
    let log_dir = job
        .log_path
        .parent()
        .with_context(|| format!("Failed to get parent dir of log file {:?}", job.log_path))?;
    fs::create_dir_all(log_dir).with_context(|| format!("Failed to create log dir {log_dir:?}"))?;
    let mut log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(job.log_path.clone())
        .with_context(|| format!("Failed to open log file {:?}", job.log_path))?;

    // Write the log file header for this execution
    lazy_static! {
        static ref DIVIDER: String = "-".repeat(80);
    }
    log_file.write_all(format!("{formatted_start_time}\n{}\n", *DIVIDER).as_bytes())?;

    // Run the command
    let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
    let process = process::Command::new(&job.shell)
        .args(["-c", &job.command])
        .stdin(process::Stdio::null())
        .stdout(clone_log_file()?)
        .stderr(clone_log_file()?)
        .spawn()
        .with_context(|| format!("Failed to run command {}", job.command))?;

    job.process = Some(process);
    drop(job);

    // Check the status periodically until it exits without holding onto the child process lock
    let mut poll_interval = Duration::from_millis(1);
    let (status_code, status_code_str) = loop {
        let mut job = job_lock.write().unwrap();

        // Attempt to terminate the process if we got a terminate signal from the terminate controller
        if terminate_controller.is_terminated() {
            let result = job.process.as_mut().unwrap().kill();

            // If the result was an InvalidInput error, it is because the process already
            // terminated, so ignore that type of error
            match result {
                Ok(_) => break (None, "terminated".to_string()),
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
        let process = job.process.as_mut().unwrap();
        let maybe_status = process
            .try_wait()
            .with_context(|| format!("Failed to get command status for command {}", job.command))?;
        drop(job);
        match maybe_status {
            // Wait briefly then loop again
            None => {
                // Wait even longer the next time with a maximum poll delay
                poll_interval = std::cmp::min(poll_interval * 2, Duration::from_secs(5));
                std::thread::sleep(poll_interval);
            }
            Some(status) => {
                break match status.code() {
                    Some(code) => (Some(code), code.to_string()),
                    None => (None, "unknown".to_string()),
                };
            }
        }
    };

    let mut job = job_lock.write().unwrap();
    job.process = None;
    drop(job);

    // Write the log file footer that contains the execution status
    log_file.write_all(format!("{}\nStatus: {status_code_str}\n\n", *DIVIDER).as_bytes())?;

    if let Some(code) = status_code {
        // Update the run status code in the database
        chron_lock
            .read()
            .unwrap()
            .db
            .lock()
            .unwrap()
            .set_run_status_code(run.id, code)?;

        if code != 0 {
            warn!("{name}: failed with exit code {code}");

            if Command::new("mailbox")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args([
                    "add",
                    format!("chron/error/{name}").as_str(),
                    format!("{name} failed with exit code {code}").as_str(),
                ])
                .spawn()
                .is_err()
            {
                warn!("Failed to write mailbox message");
            }
        }
    }

    Ok(match status_code {
        Some(code) if code != 0 => ExecStatus::Failure,
        _ => ExecStatus::Success,
    })
}

// Execute the job's command, handling retries
// Return a boolean indicating whether the command completed
pub(crate) fn exec_command(
    chron_lock: &ChronServiceLock,
    job_lock: &JobLock,
    terminate_controller: &TerminateController,
    retry_config: &RetryConfig,
) -> Result<bool> {
    let name = job_lock.read().unwrap().name.clone();
    let num_attempts = match retry_config.limit {
        Some(limit) => limit.to_string(),
        None => "unlimited".to_string(),
    };
    for attempt in 0.. {
        // Stop executing if a terminate was requested
        if terminate_controller.is_terminated() {
            // The command is only considered incomplete if it aborts
            return Ok(false);
        }

        if attempt > 0 {
            debug!("{name}: retry attempt {attempt} of {num_attempts}");
        }

        let status = exec_command_once(chron_lock, job_lock, terminate_controller)?;
        if !retry_config.should_retry(status, attempt) {
            break;
        }

        // Re-run the job after the configured delay if it is set
        if let Some(delay) = retry_config.delay {
            sleep_duration(delay)?;
        }
    }

    Ok(true)
}
