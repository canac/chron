use crate::sync_ext::RwLockExt;
use anyhow::{Context, Result};
use std::sync::{
    RwLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{RecvTimeoutError, Sender, channel},
};
use std::time::Duration;

// A TerminateController can be used to stop a running job
pub struct TerminateController {
    terminated: AtomicBool,
    tx: RwLock<Option<Sender<()>>>,
}

impl TerminateController {
    // Create a new TerminateController instance
    pub fn new() -> Self {
        Self {
            terminated: AtomicBool::new(false),
            tx: RwLock::new(None),
        }
    }

    // Terminate the controller
    pub fn terminate(&self) {
        self.terminated.store(true, Ordering::Relaxed);

        // Notify the most recent waiting wait_blocking caller that the controller has been terminated
        let mut tx_guard = self.tx.write_unpoisoned();
        let tx = tx_guard.take();
        drop(tx_guard);

        if let Some(tx) = tx {
            // Ignore send errors if wait_blocking has already returned
            let _ = tx.send(());
        }
    }

    // Return a boolean indicating whether the controller has been terminated
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Relaxed)
    }

    // Wait for the controller to be terminated, blocking for at most the specified duration
    // It is an error to call wait_blocking concurrently
    // Return a boolean indicating whether the controller has been terminated
    pub fn wait_blocking(&self, timeout: Duration) -> Result<bool> {
        if self.is_terminated() {
            return Ok(true);
        }

        let (tx, rx) = channel();
        *self.tx.write_unpoisoned() = Some(tx);

        if let err @ Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(timeout) {
            // tx was dropped because a subsequent call to wait_blocking dropped the previous value of self.tx
            err.context("wait_blocking called concurrently")?;
        };

        // Clear the transmitter so that the next call can set it again
        *self.tx.write_unpoisoned() = None;

        Ok(self.is_terminated())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread::{sleep, spawn};

    use super::*;

    const WAIT_DURATION: Duration = Duration::from_millis(1);

    #[test]
    fn test_not_terminated() {
        let terminate_controller = TerminateController::new();

        assert!(!terminate_controller.is_terminated());
    }

    #[test]
    fn test_terminated() {
        let terminate_controller = TerminateController::new();
        terminate_controller.terminate();

        assert!(terminate_controller.is_terminated());
    }

    #[test]
    fn test_wait_blocking() {
        let terminate_controller = Arc::new(TerminateController::new());

        let terminate_controller_cloned = Arc::clone(&terminate_controller);
        let handle = spawn(move || {
            sleep(WAIT_DURATION / 2);
            terminate_controller_cloned.terminate();
        });

        assert!(terminate_controller.wait_blocking(WAIT_DURATION).unwrap());

        handle.join().unwrap();
    }

    #[test]
    fn test_wait_blocking_terminated() {
        let terminate_controller = Arc::new(TerminateController::new());
        terminate_controller.terminate();

        assert!(terminate_controller.wait_blocking(WAIT_DURATION).unwrap());
    }

    #[test]
    fn test_wait_blocking_concurrent() {
        let terminate_controller = Arc::new(TerminateController::new());

        let terminate_controller_cloned = Arc::clone(&terminate_controller);
        let handle = spawn(move || {
            sleep(WAIT_DURATION / 2);
            terminate_controller_cloned
                .wait_blocking(WAIT_DURATION)
                .unwrap();
        });

        assert_eq!(
            terminate_controller
                .wait_blocking(WAIT_DURATION)
                .unwrap_err()
                .to_string(),
            "wait_blocking called concurrently"
        );

        handle.join().unwrap();
    }

    #[test]
    fn test_wait_blocking_unterminated() {
        let terminate_controller = Arc::new(TerminateController::new());

        assert!(!terminate_controller.wait_blocking(WAIT_DURATION).unwrap());
    }

    #[test]
    fn test_wait_blocking_twice() {
        let terminate_controller = Arc::new(TerminateController::new());

        assert!(
            !terminate_controller
                .wait_blocking(Duration::from_millis(1))
                .unwrap()
        );
        assert!(
            !terminate_controller
                .wait_blocking(Duration::from_millis(1))
                .unwrap()
        );
    }
}
