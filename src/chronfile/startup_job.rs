use crate::chron_service::{RetryConfig, StartupJobOptions};
use serde::{Deserialize, Deserializer};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
struct KeepAliveConfig {
    failures: Option<bool>,
    successes: Option<bool>,
    limit: Option<usize>,
    delay: Option<Duration>,
}

// Allow KeepAliveConfig to be deserialized from a boolean or a full config
fn deserialize_keep_alive<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<KeepAliveConfig, D::Error> {
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

    Ok(match KeepAliveVariant::deserialize(deserializer)? {
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
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StartupJob {
    pub command: String,
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default, deserialize_with = "deserialize_keep_alive")]
    keep_alive: KeepAliveConfig,
}

impl StartupJob {
    pub fn get_options(&self) -> StartupJobOptions {
        StartupJobOptions {
            keep_alive: RetryConfig {
                failures: self.keep_alive.failures.unwrap_or(false),
                successes: self.keep_alive.successes.unwrap_or(false),
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
    fn test_keep_alive_default() -> Result<()> {
        assert_eq!(
            toml::from_str::<StartupJob>("command = 'echo'")?
                .get_options()
                .keep_alive,
            RetryConfig {
                failures: false,
                successes: false,
                limit: None,
                delay: None,
            }
        );
        Ok(())
    }

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
