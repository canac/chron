use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct JobStatus {
    pub command: String,
    pub shell: String,
    pub schedule: Option<String>,
    pub current_run_id: Option<u32>,
    pub status: String,
}
