use chrono::{DateTime, Utc};
use cron::Schedule;

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
}
