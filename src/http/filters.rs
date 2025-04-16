#![allow(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]

use crate::format;
use askama::Result;
use chrono::{DateTime, Duration, Local};

// Format a duration as a human-readable string
pub fn duration(duration: &&Duration) -> Result<String> {
    Ok(crate::format::duration(duration))
}

// Format a date as text
pub fn date(date: &&DateTime<Local>) -> Result<String> {
    Ok(date.format("%a %h %d, %Y %r").to_string())
}

// Format a date relative to now
pub fn relative_date(date: &&DateTime<Local>) -> Result<String> {
    Ok(format::relative_date(date))
}
