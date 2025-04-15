use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct JobStatus {
    pub command: String,
    pub shell: String,
    pub schedule: Option<String>,
    pub status: String,
}
