use super::terminate_controller::TerminateController;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::time::Duration;

// Sleep until the specified timestamp, preventing oversleeping during hibernation, and aborting if the
// terminate_controller is terminated
pub fn sleep_until(
    timestamp: DateTime<Utc>,
    terminate_controller: &TerminateController,
) -> Result<()> {
    let max_sleep = Duration::from_secs(1);
    // to_std returns Err if the duration is negative, in which case we have hit the timestamp and can stop looping
    while let Ok(duration) = timestamp.signed_duration_since(Utc::now()).to_std() {
        // Block for a maximum of one minute to prevent oversleeping when the computer hibernates
        if terminate_controller.wait_blocking(std::cmp::min(duration, max_sleep))? {
            break;
        }
    }
    Ok(())
}
