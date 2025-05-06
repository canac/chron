#![allow(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]

use crate::format;
use askama::{Result, Values};
use chrono::{DateTime, Duration, Local};

// Format a duration as a human-readable string
pub fn duration(duration: &&Duration, _: &dyn Values) -> Result<String> {
    Ok(crate::format::duration(duration))
}

// Format a date as text
pub fn date(date: &&DateTime<Local>, _: &dyn Values) -> Result<String> {
    Ok(date.format("%a %h %d, %Y %r").to_string())
}

// Format a date relative to now
pub fn relative_date(date: &&DateTime<Local>, _: &dyn Values) -> Result<String> {
    Ok(format::relative_date(date))
}
