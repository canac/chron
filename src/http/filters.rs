use chrono::Duration;
use chrono_humanize::{Accuracy, HumanTime, Tense};

// Convert a duration into a human-readable string
#[allow(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
pub fn duration(duration: &&Duration) -> askama::Result<String> {
    Ok(HumanTime::from(**duration).to_text_en(Accuracy::Precise, Tense::Present))
}
