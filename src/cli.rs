use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = env!("CARGO_PKG_VERSION"),
    author = env!("CARGO_PKG_AUTHORS")
)]
pub struct Cli {
    /// HTTP server port
    #[clap(short = 'p', long, env = "PORT")]
    pub port: Option<u16>,

    /// Log fewer messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Path to the chronfile
    pub chronfile: PathBuf,
}
