use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
pub struct RunArgs {
    /// HTTP server port
    #[clap(short = 'p', long, env = "PORT", default_value = "2748")]
    pub port: u16,

    /// Log fewer messages
    #[clap(short = 'q', long)]
    pub quiet: bool,

    /// Path to the chronfile
    pub chronfile: PathBuf,
}

#[derive(Parser)]
pub struct StatusArgs {
    /// The job's name
    pub job: String,
}

#[derive(Parser)]
pub struct RunsArgs {
    /// The job's name
    pub job: String,
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
pub struct KillArgs {
    /// The job's name
    pub job: String,
}

#[derive(Parser)]
pub enum Command {
    /// Run a Chronfile
    Run(RunArgs),

    /// Print the current status of all active jobs
    Jobs,

    /// Print the job's current status
    Status(StatusArgs),

    /// Print the job's most recent runs
    Runs(RunsArgs),

    /// Print the logs of the job's most recent run
    Logs(LogsArgs),

    /// Kill the job's currently running process
    Kill(KillArgs),
}

#[derive(Parser)]
#[clap(about, version, author)]
pub struct Cli {
    /// The directory to store state and logs (defaults to a platform-specific data directory)
    #[clap(short = 'd', long, env = "CHRON_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    #[clap(subcommand)]
    pub command: Command,
}
