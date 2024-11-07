use chrono::Duration;
use std::cmp::max;

// Number of milliseconds in various time periods
const MS_SECOND: u64 = 1000;
const MS_MINUTE: u64 = MS_SECOND * 60;
const MS_HOUR: u64 = MS_MINUTE * 60;
const MS_DAY: u64 = MS_HOUR * 24;
const MS_WEEK: u64 = MS_DAY * 7;
const MS_MONTH: u64 = MS_DAY * 30;
const MS_YEAR: u64 = MS_DAY * 365;

// Convert a duration into a human-readable string
// Inspired by chrono-humanize (https://github.com/imp/chrono-humanize-rs)
#[allow(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
pub fn duration(duration: &&Duration) -> askama::Result<String> {
    Ok(match duration.num_milliseconds().unsigned_abs() {
        n if n > MS_YEAR * 3 / 2 => format!("{} years", max(n / MS_YEAR, 2)),
        n if n > MS_YEAR => String::from("1 year"),
        n if n > MS_MONTH * 3 / 2 => format!("{} months", max(n / MS_MONTH, 2)),
        n if n > MS_MONTH => String::from("1 month"),
        n if n > MS_WEEK * 3 / 2 => format!("{} weeks", max(n / MS_WEEK, 2)),
        n if n > MS_WEEK => String::from("1 week"),
        n if n > MS_DAY * 3 / 2 => format!("{} days", max(n / MS_DAY, 2)),
        n if n > MS_DAY => String::from("1 day"),
        n if n > MS_HOUR * 3 / 2 => format!("{} hours", max(n / MS_HOUR, 2)),
        n if n > MS_HOUR => String::from("1 hour"),
        n if n > MS_MINUTE * 3 / 2 => format!("{} minutes", max(n / MS_MINUTE, 2)),
        n if n > MS_MINUTE => String::from("1 minute"),
        n if n > MS_SECOND * 3 / 2 => format!("{} seconds", max(n / MS_SECOND, 2)),
        n if n > MS_SECOND => String::from("1 second"),
        1 => String::from("1 millisecond"),
        n => format!("{n} milliseconds"),
    })
}
