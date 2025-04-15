use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
pub struct RunArgs {
    /// HTTP server port
    #[clap(short = 'p', long, env = "PORT")]
    pub port: u16,

    /// Log fewer messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Path to the chronfile
    pub chronfile: PathBuf,
}

#[derive(Parser)]
pub struct LogsArgs {
    /// The job's name
    pub job: String,

    /// Print only the last n lines
    #[clap(short = 'n', long)]
    pub lines: Option<usize>,

    /// Continue printing new logs as they arrive
    #[clap(short, long)]
    pub follow: bool,
}

#[derive(Parser)]
pub enum Command {
    /// Run a Chronfile
    Run(RunArgs),

    /// Print the logs of the job's most recent run
    Logs(LogsArgs),
}

#[derive(Parser)]
#[clap(about, version, author)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: Command,
}
