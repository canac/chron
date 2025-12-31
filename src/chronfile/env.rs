use anyhow::Context;
use anyhow::Result;
use std::env::var;
use std::path::PathBuf;

pub struct Env {
    pub home_dir: PathBuf,
    pub shell: String,
}

impl Env {
    /// Construct an environment from the user's host environment
    pub fn from_host() -> Result<Self> {
        #[cfg(target_os = "windows")]
        let shell = String::from("Invoke-Expression");
        #[cfg(not(target_os = "windows"))]
        let shell = var("SHELL").context("Couldn't get $SHELL environment variable")?;

        Ok(Self {
            home_dir: var("HOME")
                .context("Couldn't get $HOME environment variable")?
                .into(),
            shell,
        })
    }

    /// Construct a mock environment for tests
    #[cfg(test)]
    pub fn mock() -> Self {
        Self {
            home_dir: PathBuf::from("/Users/user"),
            shell: "shell".to_owned(),
        }
    }
}
