use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[clap(about, version, author)]
pub struct Cli {
    /// HTTP server port
    #[clap(short = 'p', long, env = "PORT")]
    pub port: u16,

    /// Log fewer messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Path to the chronfile
    pub chronfile: PathBuf,
}
