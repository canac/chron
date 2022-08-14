use crate::chron_service::{self, StartupJobOptions};
use serde::Deserialize;
use std::time::Duration;

// Allow RawRetryConfig to be deserialized from a boolean or a full config
#[derive(Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum KeepAliveVariant {
    Simple(bool),
    Complex {
        failures: Option<bool>,
        successes: Option<bool>,
        limit: Option<usize>,
        #[serde(default, with = "humantime_serde")]
        delay: Option<Duration>,
    },
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(from = "KeepAliveVariant")]
struct KeepAliveConfig {
    failures: Option<bool>,
    successes: Option<bool>,
    limit: Option<usize>,
    delay: Option<Duration>,
}

impl From<KeepAliveVariant> for KeepAliveConfig {
    fn from(value: KeepAliveVariant) -> Self {
        match value {
            KeepAliveVariant::Simple(value) => KeepAliveConfig {
                failures: Some(value),
                successes: Some(value),
                ..Default::default()
            },
            KeepAliveVariant::Complex {
                failures,
                successes,
                limit,
                delay,
            } => KeepAliveConfig {
                failures,
                successes,
                limit,
                delay,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct StartupJob {
    pub(super) command: String,
    #[serde(default)]
    pub(super) disabled: bool,
    #[serde(default)]
    keep_alive: KeepAliveConfig,
}

impl StartupJob {
    pub fn get_options(&self) -> StartupJobOptions {
        StartupJobOptions {
            keep_alive: chron_service::RetryConfig {
                failures: self.keep_alive.failures.unwrap_or(true),
                successes: self.keep_alive.successes.unwrap_or(true),
                limit: self.keep_alive.limit,
                delay: self.keep_alive.delay,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;

    #[test]
    fn test_keep_alive() -> Result<()> {
        assert_eq!(
            toml::from_str::<StartupJob>("command = 'echo'\nkeepAlive = false")?.keep_alive,
            KeepAliveConfig {
                failures: Some(false),
                successes: Some(false),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<StartupJob>("command = 'echo'\nkeepAlive = true")?.keep_alive,
            KeepAliveConfig {
                failures: Some(true),
                successes: Some(true),
                ..Default::default()
            }
        );

        assert_eq!(
            toml::from_str::<StartupJob>(
                "command = 'echo'\nkeepAlive = { successes = true, limit = 3 }"
            )?
            .keep_alive,
            KeepAliveConfig {
                failures: None,
                successes: Some(true),
                limit: Some(3),
                delay: None,
            }
        );

        Ok(())
    }
}
