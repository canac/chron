use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, Timelike, Utc};
use cron::Schedule;
use std::time::Duration;

// Tracks when a job should run over time based on its schedule
// The schedule is interpreted in local time, but all ScheduledJob methods
// return Utc times
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

    // Return the string representation of the job's schedule
    pub fn get_schedule(&self) -> String {
        self.schedule.to_string()
    }

    // Return the date of the last time that this scheduled job ran
    pub fn prev_run(&self) -> Option<DateTime<Utc>> {
        let last_tick: DateTime<Local> = self.last_tick.into();
        // Schedule::after ignores fractional seconds, so compensate by
        // rounding fractional seconds up
        // https://github.com/zslayton/cron/issues/108
        let after = if last_tick.timestamp_subsec_nanos() == 0 {
            last_tick
        } else {
            last_tick.with_nanosecond(0).unwrap() + chrono::Duration::seconds(1)
        };
        self.schedule
            .after(&after)
            .next_back()
            .map(std::convert::Into::into)
    }

    // Return the date of the next time that this scheduled job will run
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.schedule
            .after::<Local>(&self.last_tick.into())
            .next()
            .map(std::convert::Into::into)
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
            // There is a bug in cron where reverse iterators starts counting
            // from the second rounded down, so add a second to compensate
            // https://github.com/zslayton/cron/issues/108
            .after::<Local>(
                &now.checked_add_signed(chrono::Duration::seconds(1))
                    .unwrap()
                    .into(),
            )
            // Iterating backwards in time (from newest to oldest)
            .rev()
            // Capped at `max`
            .take(max.unwrap_or(usize::MAX))
            // Until the last tick
            .take_while(|run| run > &last_tick)
            // Convert from local time to UTC
            .map(std::convert::Into::into)
            .collect::<Vec<_>>();
        // Sort by oldest to newest
        runs.reverse();
        runs
    }

    // Calculate the estimated duration between the last run and the next run
    pub fn get_current_period(&self) -> Result<Duration> {
        let mut upcoming = self.schedule.after(&Local::now());
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
    use chrono::{NaiveDate, NaiveDateTime};
    use cron::Schedule;
    use std::str::FromStr;

    use super::*;

    fn date(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        min: u32,
        sec: u32,
        milli: u32,
    ) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_milli_opt(hour, min, sec, milli)
            .unwrap()
    }

    #[test]
    fn test_period() -> Result<()> {
        let job = ScheduledJob::new(Schedule::from_str("*/5 * * * * *")?, None);
        assert_eq!(job.get_current_period()?, Duration::from_secs(5));
        Ok(())
    }

    #[test]
    fn test_prev_run_whole_seconds() {
        let start_time = date(2022, 1, 1, 0, 0, 1, 0);
        let job = ScheduledJob::new(
            Schedule::from_str("* * * * * *").unwrap(),
            Some(DateTime::<Utc>::from_utc(start_time, Utc)),
        );
        assert_eq!(
            job.prev_run().unwrap().naive_utc(),
            date(2022, 1, 1, 0, 0, 0, 0)
        );
    }

    #[test]
    fn test_prev_run_fractional_seconds() {
        let start_time = date(2022, 1, 1, 0, 0, 0, 1);
        let job = ScheduledJob::new(
            Schedule::from_str("* * * * * *").unwrap(),
            Some(DateTime::<Utc>::from_utc(start_time, Utc)),
        );
        assert_eq!(
            job.prev_run().unwrap().naive_utc(),
            date(2022, 1, 1, 0, 0, 0, 0)
        );
    }

    #[test]
    fn test_next_run_whole_seconds() {
        let start_time = date(2022, 1, 1, 0, 0, 0, 0);
        let job = ScheduledJob::new(
            Schedule::from_str("* * * * * *").unwrap(),
            Some(DateTime::<Utc>::from_utc(start_time, Utc)),
        );
        assert_eq!(
            job.next_run().unwrap().naive_utc(),
            date(2022, 1, 1, 0, 0, 1, 0)
        );
    }

    #[test]
    fn test_next_run_fractional_seconds() {
        let start_time = date(2022, 1, 1, 0, 0, 0, 1);
        let job = ScheduledJob::new(
            Schedule::from_str("* * * * * *").unwrap(),
            Some(DateTime::<Utc>::from_utc(start_time, Utc)),
        );
        assert_eq!(
            job.next_run().unwrap().naive_utc(),
            date(2022, 1, 1, 0, 0, 1, 0)
        );
    }
}
