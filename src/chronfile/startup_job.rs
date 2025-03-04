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

    // Parse a startup job and return its keep alive config
    fn parse_keep_alive(input: &'static str) -> Result<RetryConfig> {
        Ok(toml::from_str::<StartupJob>(input)?
            .get_options()
            .keep_alive)
    }

    #[test]
    fn test_keep_alive_omitted() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'")?,
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
    fn test_keep_alive_false() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = false")?,
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
    fn test_keep_alive_true() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = true")?,
            RetryConfig {
                failures: true,
                successes: true,
                limit: None,
                delay: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_keep_alive_limit() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = { successes = true, limit = 3 }")?,
            RetryConfig {
                failures: false,
                successes: true,
                limit: Some(3),
                delay: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_keep_alive_delay() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = { successes = true, delay = '10m' }")?,
            RetryConfig {
                failures: false,
                successes: true,
                limit: None,
                delay: Some(Duration::from_secs(600)),
            }
        );
        Ok(())
    }
}
