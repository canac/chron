use std::sync::{Arc, RwLock};

// A TerminateController can be used to stop a running job
#[derive(Clone)]
pub struct TerminateController {
    terminated: Arc<RwLock<bool>>,
}

impl TerminateController {
    // Create a new TerminateController instance
    pub fn new() -> Self {
        TerminateController {
            terminated: Arc::new(RwLock::new(false)),
        }
    }

    // Terminate the controller
    pub fn terminate(&self) {
        *self.terminated.write().unwrap() = true;
    }

    // Return a boolean indicating whether the controller has been terminated
    pub fn is_terminated(&self) -> bool {
        *self.terminated.read().unwrap()
    }
}
