use serde::{
    Deserialize,
    de::{Deserializer, Error},
};
use std::time::Duration;

#[allow(clippy::unnecessary_wraps)]
fn default_limit() -> Option<usize> {
    Some(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryConfig {
    pub limit: Option<usize>,
    pub delay: Option<Duration>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            delay: None,
        }
    }
}

impl<'de> Deserialize<'de> for RetryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawConfig {
            #[serde(default = "default_limit")]
            limit: Option<usize>,
            #[serde(default, with = "humantime_serde")]
            delay: Option<Duration>,
        }

        let raw = RawConfig::deserialize(deserializer)?;
        if raw.limit == Some(0) && raw.delay.is_some() {
            return Err(Error::custom("`delay` cannot be set when `limit` is zero"));
        }

        Ok(Self {
            limit: raw.limit,
            delay: raw.delay,
        })
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::time::Duration;
    use toml::de::Error;

    use super::*;

    #[derive(Deserialize)]
    struct TestStruct {
        retry: RetryConfig,
    }

    /// Parse a startup job and return its keep alive config
    fn parse_retry(input: &'static str) -> std::result::Result<RetryConfig, Error> {
        Ok(toml::from_str::<TestStruct>(input)?.retry)
    }

    #[test]
    fn test_retry_empty() -> Result<()> {
        assert_eq!(
            parse_retry("retry = {}")?,
            RetryConfig {
                limit: Some(0),
                delay: None,
            },
        );
        Ok(())
    }

    #[test]
    fn test_retry_limit() -> Result<()> {
        assert_eq!(
            parse_retry("retry = { limit = 3 }")?,
            RetryConfig {
                limit: Some(3),
                delay: None,
            },
        );
        Ok(())
    }

    #[test]
    fn test_retry_delay() -> Result<()> {
        assert_eq!(
            parse_retry("retry = { limit = 3, delay = '10m' }")?,
            RetryConfig {
                limit: Some(3),
                delay: Some(Duration::from_secs(600)),
            },
        );
        Ok(())
    }

    #[test]
    fn test_retry_delay_without_limit() {
        assert_eq!(
            parse_retry("retry = { delay = '10m' }")
                .unwrap_err()
                .message(),
            "`delay` cannot be set when `limit` is zero"
        );
    }
}
