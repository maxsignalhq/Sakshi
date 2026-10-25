//! `sakshi` binary entry point.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Parser;
use sakshi_tui::RunConfig;

/// The witness for your AI coding agents.
#[derive(Debug, Parser)]
#[command(name = "sakshi", version, about)]
struct Cli {
    /// Program to run, with its arguments. Defaults to your shell.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

#[cfg(windows)]
fn default_shell() -> String {
    "powershell.exe".to_string()
}

#[cfg(not(windows))]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Turn the command line into what the TUI should run.
fn run_config(command: Vec<String>, cwd: std::path::PathBuf) -> RunConfig {
    let mut parts = command.into_iter();
    let program = parts.next().unwrap_or_else(default_shell);
    let name = Path::new(&program)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.clone());
    RunConfig {
        name,
        program,
        args: parts.collect(),
        cwd,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("cannot determine the current directory")?;
    sakshi_tui::run(run_config(cli.command, cwd))
        .await
        .context("sakshi exited with an error")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn explicit_command_is_split_into_program_and_args() {
        let cfg = run_config(
            vec!["/usr/bin/claude".into(), "--resume".into()],
            PathBuf::from("."),
        );
        assert_eq!(cfg.program, "/usr/bin/claude");
        assert_eq!(cfg.args, vec!["--resume".to_string()]);
        assert_eq!(cfg.name, "claude");
    }

    #[test]
    fn empty_command_falls_back_to_shell() {
        let cfg = run_config(Vec::new(), PathBuf::from("."));
        assert_eq!(cfg.program, default_shell());
        assert!(cfg.args.is_empty());
    }
}
