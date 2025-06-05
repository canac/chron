#![allow(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]

use crate::format;
use askama::{Result, Values};
use chrono::{DateTime, Duration, Local, TimeZone};

/// Format a duration as a human-readable string
pub fn duration(duration: &&Duration, _: &dyn Values) -> Result<String> {
    Ok(crate::format::duration(duration))
}

/// Format a date as text
pub fn date<Tz>(date: &&DateTime<Tz>, _: &dyn Values) -> Result<String>
where
    Tz: TimeZone,
    DateTime<Tz>: Copy,
    DateTime<Local>: From<DateTime<Tz>>,
{
    Ok(DateTime::<Local>::from(**date)
        .format("%a %h %d, %Y %r")
        .to_string())
}

/// Format a date relative to now
pub fn relative_date<Tz>(date: &&DateTime<Tz>, _: &dyn Values) -> Result<String>
where
    Tz: TimeZone,
    DateTime<Tz>: Copy,
    DateTime<Local>: From<DateTime<Tz>>,
{
    Ok(format::relative_date(&DateTime::<Local>::from(**date)))
}
