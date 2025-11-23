use super::RetryConfig;
use chrono::{DateTime, Utc};

pub struct Attempt<'t> {
    pub scheduled_time: &'t DateTime<Utc>,
    pub attempt: usize,
}

impl Attempt<'_> {
    /// Given a command's status code and its retry config, determine when its next attempt should be made, if any
    pub fn next_attempt(
        &self,
        status_code: Option<i32>,
        retry_config: &RetryConfig,
    ) -> Option<DateTime<Utc>> {
        if retry_config
            .limit
            .is_some_and(|limit| self.attempt >= limit)
        {
            // There are no more remaining attempts
            return None;
        }

        if status_code == Some(0) {
            None
        } else {
            Some(Utc::now() + retry_config.delay.unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS_CODE: Option<i32> = Some(0);
    const FAILURE_CODE: Option<i32> = Some(1);

    #[test]
    fn test_next_attempt_no_retries() {
        let retry_config = RetryConfig {
            limit: Some(0),
            delay: None,
        };

        let scheduled_time = &Utc::now();
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(SUCCESS_CODE, &retry_config)
            .is_none()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_none()
        );
    }

    #[test]
    fn test_next_attempt() {
        let retry_config = RetryConfig {
            limit: None,
            delay: None,
        };

        let scheduled_time = &Utc::now();
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(SUCCESS_CODE, &retry_config)
            .is_none()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
    }

    #[test]
    fn test_next_attempt_limited_retries() {
        let retry_config = RetryConfig {
            limit: Some(2),
            delay: None,
        };

        let scheduled_time = &Utc::now();
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: 1,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: 2,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_none()
        );
    }

    #[test]
    fn test_next_attempt_unlimited_retries() {
        let retry_config = RetryConfig {
            limit: None,
            delay: None,
        };

        let scheduled_time = &Utc::now();
        assert!(
            Attempt {
                scheduled_time,
                attempt: 0,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: 1000,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
        assert!(
            Attempt {
                scheduled_time,
                attempt: usize::MAX,
            }
            .next_attempt(FAILURE_CODE, &retry_config)
            .is_some()
        );
    }
}
