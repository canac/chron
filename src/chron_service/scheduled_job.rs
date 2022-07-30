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

    // Tick and return a list of the elapsed runs since the last tick. The
    // vector contain the `max` most recent items, arranged from oldest to newest
    pub fn tick(&mut self, max: Option<usize>) -> Vec<DateTime<Utc>> {
        let now = Utc::now();
        let last_tick = self.last_tick;
        self.last_tick = now;

        let mut runs = self
            .schedule
            // Get the runs from now
            .after(&now)
            // Iterating backwards in time (from newest to oldest)
            .rev()
            // Capped at `max`
            .take(max.unwrap_or(usize::MAX))
            // Until the last tick
            .take_while(|run| run > &last_tick)
            .collect::<Vec<_>>();
        // Sort by oldest to newest
        runs.reverse();
        runs
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
