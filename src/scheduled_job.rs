use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use cron::Schedule;
use std::time::Duration;

pub struct ScheduledJob {
    schedule: Schedule,
    last_tick: DateTime<Utc>,
}

impl ScheduledJob {
    // Create a new scheduled job
    pub fn new(schedule: Schedule, last_run: Option<DateTime<Utc>>) -> Self {
        ScheduledJob {
            schedule,
            last_tick: last_run.unwrap_or_else(Utc::now),
        }
    }

    // Return the date of the next time that this scheduled job will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.schedule.after(&self.last_tick).next()
    }

    // Determine whether the job should be run
    pub fn tick(&mut self) -> bool {
        let now = Utc::now();
        let should_run = self.next_run().map_or(false, |next_run| next_run <= now);
        self.last_tick = now;
        should_run
    }

    // Calculate the estimated duration between the last run and the next run
    pub fn get_current_period(&self) -> Result<Duration> {
        let mut upcoming = self.schedule.after(&Utc::now());
        let last = upcoming
            .next_back()
            .ok_or_else(|| anyhow!("Failed to get previous run"))?;
        let next = upcoming
            .next()
            .ok_or_else(|| anyhow!("Failed to get next run"))?;
        next.signed_duration_since(last)
            .to_std()
            .context("Failed to convert duration")
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use cron::Schedule;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_period() -> Result<()> {
        let job = ScheduledJob::new(Schedule::from_str("*/5 * * * * *")?, None);
        assert_eq!(job.get_current_period()?, Duration::from_secs(5));
        Ok(())
    }
}
