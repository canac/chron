use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, Utc};
use cron::Schedule;
use std::time::Duration;

#[cfg_attr(test, derive(Debug, Eq, PartialEq))]
pub struct ElapsedRuns {
    /// The oldest (i.e. furthest in the past) elapsed run
    pub oldest: DateTime<Utc>,

    /// The newest (i.e. closest to the present) elapsed run
    pub newest: DateTime<Utc>,
}

// Tracks when a job should run over time based on its schedule
// The schedule is interpreted in local time, but all ScheduledJob methods
// return Utc times
pub struct ScheduledJob {
    schedule: Schedule,
    last_tick: DateTime<Utc>,
}

impl ScheduledJob {
    /// Create a new scheduled job
    pub fn new(schedule: Schedule, resume_at: DateTime<Utc>) -> Self {
        Self {
            schedule,
            last_tick: resume_at,
        }
    }

    /// Return the date of the next time that this scheduled job will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.schedule
            .after::<Local>(&self.last_tick.into())
            .next()
            .map(std::convert::Into::into)
    }

    /// Tick and return the elapsed runs since the last tick, if any
    pub fn tick(&mut self, now: DateTime<Utc>) -> Option<ElapsedRuns> {
        let last_tick = std::mem::replace(&mut self.last_tick, now);

        // Get all the runs from the last tick until now
        let mut iter = self
            .schedule
            .after::<Local>(&last_tick.into())
            .take_while(|run| run <= &now);

        let oldest = iter.next().map(std::convert::Into::into);
        oldest.map(|oldest| ElapsedRuns {
            oldest,
            newest: iter.last().map_or(oldest, std::convert::Into::into),
        })
    }

    /// Calculate the estimated duration between the last run and the next run
    pub fn get_current_period(&self, now: &DateTime<Local>) -> Result<Duration> {
        let mut upcoming = self.schedule.after(now);
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
    use chrono::{NaiveDate, Timelike};
    use cron::Schedule;
    use std::str::FromStr;

    use super::*;

    fn now() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2022, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    }

    #[test]
    fn test_period() -> Result<()> {
        let now = now();
        let job = ScheduledJob::new(Schedule::from_str("*/5 * * * * *")?, now);
        assert_eq!(job.get_current_period(&now.into())?, Duration::from_secs(5));
        Ok(())
    }

    #[test]
    fn test_next_run_whole_seconds() {
        let start_time = now();
        let job = ScheduledJob::new(Schedule::from_str("* * * * * *").unwrap(), start_time);
        assert_eq!(job.next_run().unwrap(), now().with_second(1).unwrap());
    }

    #[test]
    fn test_next_run_fractional_seconds() {
        let start_time = now().with_nanosecond(1).unwrap();
        let job = ScheduledJob::new(Schedule::from_str("* * * * * *").unwrap(), start_time);
        assert_eq!(job.next_run().unwrap(), now().with_second(1).unwrap());
    }

    #[test]
    fn test_tick_no_run() {
        let start_time = now();
        let mut job = ScheduledJob::new(Schedule::from_str("* * * * * *").unwrap(), start_time);
        assert_eq!(job.tick(now()), None);
    }

    #[test]
    fn test_tick_fractional_second() {
        let start_time = now();
        let mut job = ScheduledJob::new(Schedule::from_str("* * * * * *").unwrap(), start_time);
        let scheduled_time = now().with_second(1).unwrap();
        assert_eq!(
            job.tick(scheduled_time.with_nanosecond(1).unwrap())
                .unwrap(),
            ElapsedRuns {
                oldest: scheduled_time,
                newest: scheduled_time,
            },
        );
    }

    #[test]
    fn test_tick_skipped_runs() {
        let start_time = now();
        let mut job = ScheduledJob::new(Schedule::from_str("* * * * * *").unwrap(), start_time);
        assert_eq!(job.tick(now()), None);
        assert_eq!(
            job.tick(now().with_second(5).unwrap()).unwrap(),
            ElapsedRuns {
                oldest: now().with_second(1).unwrap(),
                newest: now().with_second(5).unwrap()
            },
        );
    }
}
