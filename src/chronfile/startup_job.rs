use crate::chron_service::{RetryConfig, StartupJobOptions};
use serde::{Deserialize, Deserializer, de::Error};
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

    match KeepAliveVariant::deserialize(deserializer)? {
        KeepAliveVariant::Simple(value) => Ok(KeepAliveConfig {
            failures: Some(value),
            successes: Some(value),
            ..Default::default()
        }),
        KeepAliveVariant::Complex {
            failures,
            successes,
            limit,
            delay,
        } => {
            if failures.is_none() && successes.is_none() && (limit.is_some() || delay.is_some()) {
                return Err(D::Error::custom(
                    "limit or delay cannot be provided if failures or successes are omitted",
                ));
            }

            Ok(KeepAliveConfig {
                failures,
                successes,
                limit,
                delay,
            })
        }
    }
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
    use toml::de::Error;

    use super::*;

    // Parse a startup job and return its keep alive config
    fn parse_keep_alive(input: &'static str) -> std::result::Result<RetryConfig, Error> {
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
    fn test_keep_alive_empty() -> Result<()> {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = {}")?,
            RetryConfig {
                failures: false,
                successes: false,
                limit: None,
                delay: None,
            },
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

    #[test]
    fn test_keep_alive_invalid() {
        assert_eq!(
            parse_keep_alive("command = 'echo'\nkeepAlive = { delay = '10m' }")
                .unwrap_err()
                .message(),
            "limit or delay cannot be provided if failures or successes are omitted",
        );
    }
}
