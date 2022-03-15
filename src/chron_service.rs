use crate::job_scheduler::{Job, JobScheduler};
use anyhow::{bail, Context, Result};
use cron::Schedule;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;

pub struct ChronService<'job> {
    log_dir: PathBuf,
    scheduled_jobs: HashMap<String, Job<'job>>,
    scheduler: JobScheduler<'job>,
}

impl<'job> ChronService<'job> {
    // Create a new ChronService instance
    pub fn new(chron_dir: &Path) -> Self {
        ChronService {
            log_dir: chron_dir.join("logs"),
            scheduled_jobs: HashMap::new(),
            scheduler: JobScheduler::new(),
        }
    }

    // Add a new command to be run on the given schedule
    pub fn add<'cmd>(
        &mut self,
        name: &'job str,
        schedule_expression: &str,
        command: &'cmd str,
    ) -> Result<()>
    where
        'cmd: 'job,
    {
        if self.scheduled_jobs.contains_key(name) {
            bail!("A job with the name {name} already exists")
        }

        let schedule = Schedule::from_str(schedule_expression).with_context(|| {
            format!("Failed to parse schedule expression {schedule_expression}")
        })?;
        let log_dir = self.log_dir.clone();
        let job = Job::new(schedule, move || {
            Self::exec_command(&log_dir, name, command).unwrap()
        });
        self.scheduler.add(job);
        // self.scheduled_jobs.insert(name.to_string(), job);

        Ok(())
    }

    // Start the chron service, running all scripts on their specified interval
    // Note that this function starts an infinite loop and will never return
    pub fn run(&mut self) {
        loop {
            self.scheduler.tick();
            std::thread::sleep(self.scheduler.time_till_next_job());
        }
    }

    // Helper to execute the specified command
    fn exec_command(log_dir: &Path, name: &str, command: &str) -> Result<()> {
        let start_time = chrono::Local::now();
        let formatted_start_time = start_time.to_rfc3339();

        println!("{formatted_start_time} Running {name}: {command}");

        // Open the log file, creating the directory if necessary
        fs::create_dir_all(log_dir)
            .with_context(|| format!("Failed to create log dir {log_dir:?}"))?;
        let mut log_path = log_dir.join(name);
        log_path.set_extension("log");
        let mut log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path.clone())
            .with_context(|| format!("Failed to open log file {log_path:?}"))?;

        // Write the log file header for this execution
        let divider: &'static str = "----------------------------------------------------------------------------------------------------";
        log_file.write_all(format!("{formatted_start_time}\n{divider}\n").as_bytes())?;

        // Run the command
        let clone_log_file = || log_file.try_clone().context("Failed to clone log file");
        let status = process::Command::new("sh")
            .args(["-c", command])
            .stdin(process::Stdio::null())
            .stdout(clone_log_file()?)
            .stderr(clone_log_file()?)
            .status()
            .with_context(|| format!("Failed to run command {command}"))?;

        // Write the log file footer that contains the execution status
        let status_code = match status.code() {
            Some(code) => code.to_string(),
            None => "unknown".to_string(),
        };
        log_file.write_all(format!("{divider}\nStatus: {status_code}\n\n").as_bytes())?;

        Ok(())
    }
}
