use super::sleep::sleep_duration;
use super::{Job, RetryConfig};
use crate::chron_service::{ChronServiceLock, Process};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use std::fs;
use std::io::Write;
use std::process::{self, Command, Stdio};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum ExecStatus {
    Success,
    Failure,
    Aborted,
}

// Helper to execute the specified command without retries
fn exec_command_once(chron_lock: &ChronServiceLock, job: &Arc<Job>) -> Result<ExecStatus> {
    static DIVIDER: LazyLock<String> = LazyLock::new(|| "-".repeat(80));

    // Don't run the job at all if it is supposed to be terminated
    if job.terminate_controller.is_terminated() {
        return Ok(ExecStatus::Aborted);
    }

    let start_time = chrono::Local::now();
    let formatted_start_time = start_time.to_rfc3339();

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

    let mut process_guard = job.running_process.write().unwrap();
    *process_guard = Some(Process {
        child_process: process,
        run_id: run.id,
    });
    drop(process_guard);

    // Check the status periodically until it exits without holding onto the child process lock
    let (status_code, status_code_str) = poll_exit_status(job)?;

    let mut process_guard = job.running_process.write().unwrap();
    process_guard.take();
    drop(process_guard);

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

// Check the status of a job until it exits without holding onto the child process lock
// The first element in the tuple is the exit status if available and the second element is
// a human-readable representation of the exit status
fn poll_exit_status(job: &Arc<Job>) -> Result<(Option<i32>, String)> {
    // Check the status periodically until it exits without holding onto the child process lock
    let mut poll_interval = Duration::from_millis(1);
    loop {
        let mut process_guard = job.running_process.write().unwrap();
        let process = &mut process_guard
            .as_mut()
            .expect("process should not be None")
            .child_process;

        // Attempt to terminate the process if we got a terminate signal from the terminate controller
        if job.terminate_controller.is_terminated() {
            let result = process.kill();

            // If the result was an InvalidInput error, it is because the process already
            // terminated, so ignore that type of error
            match result {
                Ok(()) => return Ok((None, String::from("terminated"))),
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
        let maybe_status = process
            .try_wait()
            .with_context(|| format!("Failed to get command status for command {}", job.command))?;
        drop(process_guard);
        match maybe_status {
            // Wait briefly then loop again
            None => {
                // Wait even longer the next time with a maximum poll delay
                poll_interval = std::cmp::min(poll_interval * 2, Duration::from_secs(5));
                std::thread::sleep(poll_interval);
            }
            Some(status) => {
                return Ok(status.code().map_or_else(
                    || (None, String::from("unknown")),
                    |code| (Some(code), code.to_string()),
                ));
            }
        }
    }
}

// Execute the job's command, handling retries
// Return a boolean indicating whether the command completed
pub fn exec_command(
    chron_lock: &ChronServiceLock,
    job: &Arc<Job>,
    retry_config: &RetryConfig,
) -> Result<bool> {
    let name = job.name.clone();
    let num_attempts = retry_config
        .limit
        .map_or_else(|| String::from("unlimited"), |limit| limit.to_string());
    for attempt in 0.. {
        // Stop executing if a terminate was requested
        if job.terminate_controller.is_terminated() {
            // The command is only considered incomplete if it aborts
            return Ok(false);
        }

        if attempt > 0 {
            debug!("{name}: retry attempt {attempt} of {num_attempts}");
        }

        let status = exec_command_once(chron_lock, job)?;
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
