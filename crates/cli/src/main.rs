//! `sakshi` binary entry point.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod hook;
mod logging;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sakshi_tui::RunConfig;

/// The witness for your AI coding agents.
#[derive(Debug, Parser)]
#[command(
    name = "sakshi",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    subcommand: Option<Sub>,

    /// Do not show desktop notifications.
    #[arg(long)]
    no_notify: bool,

    /// Program to run, with its arguments. Defaults to your shell.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// Internal: called by an agent's hooks to report to a running sakshi.
    #[command(hide = true)]
    Hook {
        /// Which agent is calling, e.g. `claude`.
        agent: String,
    },
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
fn run_config(command: Vec<String>, cwd: PathBuf, notifications: bool) -> RunConfig {
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
        shell: default_shell(),
        notifications,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Sub::Hook { agent }) = cli.subcommand {
        // Hooks must never fail, print, or start a UI.
        hook::run(&agent).await;
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        bail!("sakshi needs an interactive terminal");
    }
    logging::init();
    let cwd = std::env::current_dir().context("cannot determine the current directory")?;
    sakshi_tui::run(run_config(cli.command, cwd, !cli.no_notify))
        .await
        .context("sakshi exited with an error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_command_is_split_into_program_and_args() {
        let cfg = run_config(
            vec!["/usr/bin/claude".into(), "--resume".into()],
            PathBuf::from("."),
            true,
        );
        assert_eq!(cfg.program, "/usr/bin/claude");
        assert_eq!(cfg.args, vec!["--resume".to_string()]);
        assert_eq!(cfg.name, "claude");
    }

    #[test]
    fn empty_command_falls_back_to_shell() {
        let cfg = run_config(Vec::new(), PathBuf::from("."), false);
        assert_eq!(cfg.program, default_shell());
        assert_eq!(cfg.shell, default_shell());
        assert!(cfg.args.is_empty());
        assert!(!cfg.notifications);
    }

    #[test]
    fn no_notify_flag_is_parsed() {
        let cli = Cli::try_parse_from(["sakshi", "--no-notify"]).unwrap();
        assert!(cli.no_notify);
        let cli = Cli::try_parse_from(["sakshi", "claude", "--no-notify"]).unwrap();
        // After the program name, flags belong to the program.
        assert!(!cli.no_notify);
        assert_eq!(cli.command, ["claude", "--no-notify"]);
    }

    #[test]
    fn parses_hook_subcommand_and_plain_commands() {
        let cli = Cli::try_parse_from(["sakshi", "hook", "claude"]).unwrap();
        assert!(matches!(cli.subcommand, Some(Sub::Hook { ref agent }) if agent == "claude"));
        let cli = Cli::try_parse_from(["sakshi", "claude", "--resume"]).unwrap();
        assert!(cli.subcommand.is_none());
        assert_eq!(cli.command, ["claude", "--resume"]);
        let cli = Cli::try_parse_from(["sakshi"]).unwrap();
        assert!(cli.subcommand.is_none() && cli.command.is_empty());
    }
}
