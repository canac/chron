#[path = "src/cli.rs"]
mod cli;

use clap::CommandFactory;
use cli::Cli;
use std::fs::{create_dir_all, write};
use std::io::Result;
use std::path::PathBuf;

fn main() {
    // Don't rebuild when the generated completions change
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli.rs");

    generate_completions().unwrap();
    generate_manpage().unwrap();
}

fn generate_completions() -> Result<()> {
    use clap_complete::generate_to;
    use clap_complete::shells::{Bash, Elvish, Fish, PowerShell, Zsh};

    let cmd = &mut Cli::command();
    let bin_name = String::from(cmd.get_name());
    let out_dir = &PathBuf::from("contrib/completions");

    create_dir_all(out_dir)?;
    generate_to(Bash, cmd, &bin_name, out_dir)?;
    generate_to(Elvish, cmd, &bin_name, out_dir)?;
    generate_to(Fish, cmd, &bin_name, out_dir)?;
    generate_to(PowerShell, cmd, &bin_name, out_dir)?;
    generate_to(Zsh, cmd, &bin_name, out_dir)?;

    Ok(())
}

fn generate_manpage() -> Result<()> {
    let out_dir = &PathBuf::from("man/man1");
    create_dir_all(out_dir)?;

    let mut buffer: Vec<u8> = Default::default();
    clap_mangen::Man::new(Cli::command()).render(&mut buffer)?;
    write(out_dir.join("chron.1"), buffer)?;

    Ok(())
}
