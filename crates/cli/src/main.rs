//! `sakshi` binary entry point.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod config;
mod history;
mod hook;
mod logging;
mod setup;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sakshi_core::TrustConfig;
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

    /// Do not write activity logs.
    #[arg(long)]
    no_log: bool,

    /// Run the first session in its own git worktree on a new branch, so it
    /// cannot touch your checkout. Nothing is merged or removed for you.
    #[arg(long)]
    worktree: bool,

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
    /// One-time setup for an agent that needs it (currently Codex).
    Setup {
        /// Which agent to set up, e.g. `codex`.
        agent: String,
        /// Undo the setup.
        #[arg(long, conflicts_with = "status")]
        remove: bool,
        /// Only show whether the agent is set up; change nothing.
        #[arg(long)]
        status: bool,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// List recorded sessions, newest first.
    Sessions {
        /// How many sessions to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Summarise a recorded session: trust verdict, findings and what happened.
    Report {
        /// Session id, a unique prefix of one, or `latest`.
        #[arg(default_value = "latest")]
        session: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = history::Format::Md)]
        format: history::Format,
    },
    /// Replay a recorded session's timeline.
    Replay {
        /// Session id, a unique prefix of one, or `latest`.
        #[arg(default_value = "latest")]
        session: String,
        /// Play at this many times real speed; 0 prints everything at once.
        #[arg(long, default_value_t = 0.0)]
        speed: f64,
    },
}

/// Where the activity logs of past sessions live.
fn sessions_dir() -> Result<PathBuf> {
    let data = sakshi_store::data_dir().context("cannot locate the data directory")?;
    Ok(sakshi_store::sessions_dir(&data))
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
fn run_config(
    command: Vec<String>,
    cwd: PathBuf,
    notifications: bool,
    data_dir: Option<PathBuf>,
    trust: TrustConfig,
) -> RunConfig {
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
        data_dir,
        trust,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.subcommand {
        Some(Sub::Hook { agent }) => {
            // Hooks must never fail, print, or start a UI.
            hook::run(&agent).await;
            return Ok(());
        }
        Some(Sub::Setup {
            agent,
            remove,
            status,
            yes,
        }) => {
            return setup::run(
                &agent,
                setup::Options {
                    remove,
                    status,
                    yes,
                },
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
            );
        }
        Some(Sub::Sessions { limit }) => {
            return history::sessions(&sessions_dir()?, limit, &mut std::io::stdout().lock());
        }
        Some(Sub::Report { session, format }) => {
            return history::report(
                &sessions_dir()?,
                &session,
                format,
                &mut std::io::stdout().lock(),
            );
        }
        Some(Sub::Replay { session, speed }) => {
            return history::replay(
                &sessions_dir()?,
                &session,
                speed,
                &mut std::io::stdout().lock(),
            )
            .await;
        }
        None => {}
    }

    if !std::io::stdin().is_terminal() {
        bail!("sakshi needs an interactive terminal");
    }
    // Read the config before taking over the terminal so errors print plainly.
    let trust = config::load(sakshi_store::config_path().as_deref())?;
    logging::init();
    let mut cwd = std::env::current_dir().context("cannot determine the current directory")?;
    let worktree = if cli.worktree {
        let tag = worktree_tag();
        let wt = sakshi_git::add_worktree(&cwd, &tag)
            .with_context(|| "cannot create a worktree (needs a git repository with a commit)")?;
        cwd = wt.cwd.clone();
        Some(wt)
    } else {
        None
    };
    let data_dir = if cli.no_log {
        None
    } else {
        sakshi_store::data_dir()
    };
    let result = sakshi_tui::run(run_config(
        cli.command,
        cwd,
        !cli.no_notify,
        data_dir,
        trust,
    ))
    .await
    .context("sakshi exited with an error");
    if let Some(wt) = &worktree {
        println!("{}", worktree_notice(wt));
    }
    result
}

/// A short unique suffix for a worktree and its branch.
fn worktree_tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "{:06x}",
        (nanos ^ std::process::id().rotate_left(16)) & 0xff_ffff
    )
}

/// What to tell the user after a `--worktree` session: where the work is and
/// how to take it or drop it. Sakshi does neither.
fn worktree_notice(wt: &sakshi_git::Worktree) -> String {
    format!(
        "Worktree kept at {path} on branch {branch}.\n\
         Take the work:  git -C {repo} merge {branch}\n\
         Drop it:        git -C {repo} worktree remove --force {path} && git -C {repo} branch -D {branch}",
        path = wt.path.display(),
        repo = wt.repo.display(),
        branch = wt.branch,
    )
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
            None,
            TrustConfig::default(),
        );
        assert_eq!(cfg.program, "/usr/bin/claude");
        assert_eq!(cfg.args, vec!["--resume".to_string()]);
        assert_eq!(cfg.name, "claude");
    }

    #[test]
    fn empty_command_falls_back_to_shell() {
        let cfg = run_config(
            Vec::new(),
            PathBuf::from("."),
            false,
            None,
            TrustConfig::default(),
        );
        assert_eq!(cfg.program, default_shell());
        assert_eq!(cfg.shell, default_shell());
        assert!(cfg.args.is_empty());
        assert!(!cfg.notifications);
    }

    #[test]
    fn history_subcommands_parse() {
        let cli = Cli::try_parse_from(["sakshi", "report"]).unwrap();
        assert!(matches!(
            cli.subcommand,
            Some(Sub::Report { ref session, format: history::Format::Md }) if session == "latest"
        ));
        let cli = Cli::try_parse_from(["sakshi", "report", "ab12", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.subcommand,
            Some(Sub::Report { ref session, format: history::Format::Json }) if session == "ab12"
        ));
        let cli = Cli::try_parse_from(["sakshi", "replay", "--speed", "5"]).unwrap();
        assert!(matches!(cli.subcommand, Some(Sub::Replay { speed, .. }) if speed == 5.0));
        let cli = Cli::try_parse_from(["sakshi", "sessions", "--limit", "3"]).unwrap();
        assert!(matches!(cli.subcommand, Some(Sub::Sessions { limit: 3 })));
        assert!(Cli::try_parse_from(["sakshi", "report", "--format", "xml"]).is_err());
    }

    #[test]
    fn worktree_flag_belongs_to_sakshi_only_before_the_program() {
        let cli = Cli::try_parse_from(["sakshi", "--worktree", "claude"]).unwrap();
        assert!(cli.worktree);
        assert_eq!(cli.command, ["claude"]);
        let cli = Cli::try_parse_from(["sakshi", "claude", "--worktree"]).unwrap();
        assert!(!cli.worktree);
        assert_eq!(cli.command, ["claude", "--worktree"]);
        assert!(!Cli::try_parse_from(["sakshi", "claude"]).unwrap().worktree);
    }

    #[test]
    fn the_worktree_notice_names_the_path_branch_and_both_commands() {
        let wt = sakshi_git::Worktree {
            repo: PathBuf::from("/w/app"),
            path: PathBuf::from("/w/app-sakshi-ab12cd"),
            cwd: PathBuf::from("/w/app-sakshi-ab12cd"),
            branch: "sakshi/ab12cd".into(),
        };
        let text = worktree_notice(&wt);
        assert!(text.contains("/w/app-sakshi-ab12cd on branch sakshi/ab12cd"));
        assert!(text.contains("git -C /w/app merge sakshi/ab12cd"));
        assert!(text.contains("worktree remove"));
        assert_eq!(worktree_tag().len(), 6);
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
