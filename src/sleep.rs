use anyhow::Result;
use chrono::{DateTime, Utc};
use std::time::Duration;

// Sleep for the specified duration, preventing oversleeping during hibernation
pub fn sleep_duration(duration: Duration) -> Result<()> {
    sleep_until(Utc::now() + chrono::Duration::from_std(duration)?);
    Ok(())
}

// Sleep until the specified timestamp, preventing oversleeping during hibernation
pub fn sleep_until(timestamp: DateTime<Utc>) {
    let max_sleep = Duration::from_secs(60);
    // to_std returns Err if the duration is negative, in which case we
    // have hit the timestamp and cam stop looping
    while let Ok(duration) = timestamp.signed_duration_since(Utc::now()).to_std() {
        // Sleep for a maximum of one minute to prevent oversleeping when
        // the computer hibernates
        std::thread::sleep(std::cmp::min(duration, max_sleep));
    }
}
