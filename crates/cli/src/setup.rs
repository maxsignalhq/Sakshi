//! `sakshi setup <agent>`: the one place Sakshi registers anything with an
//! agent, and only when the user asks.
//!
//! Codex runs only hooks the user has approved, so its hooks cannot be injected
//! per session. Setup registers Sakshi's fixed plugin through Codex's own CLI
//! (`codex plugin ...`); it never edits Codex's files itself. It asks first,
//! and `--remove` undoes it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use sakshi_adapters::codex_plugin::{self, Status};

/// What the user asked `sakshi setup` to do.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Undo setup instead of doing it.
    pub remove: bool,
    /// Only report the current state; change nothing.
    pub status: bool,
    /// Do not ask for confirmation.
    pub yes: bool,
}

/// The parts of Codex that setup touches, so the logic can be tested without it.
trait CodexCli {
    /// Whether `codex` can be run.
    fn available(&self) -> bool;
    /// Run `codex <args>`, failing with its error output.
    fn run(&self, args: &[&str]) -> Result<()>;
    /// Text of Codex's `config.toml`, or empty if there is none. Read-only.
    fn config_text(&self) -> String;
}

struct RealCodex;

impl CodexCli for RealCodex {
    fn available(&self) -> bool {
        Command::new("codex")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn run(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("codex")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .context("could not run codex")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        bail!("`codex {}` failed: {}", args.join(" "), detail.trim())
    }

    fn config_text(&self) -> String {
        codex_home()
            .map(|home| home.join("config.toml"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    }
}

/// Codex's home: `CODEX_HOME`, else `~/.codex`.
fn codex_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".codex"))
}

/// Run `sakshi setup <agent>`.
pub fn run(
    agent: &str,
    options: Options,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    match agent.to_ascii_lowercase().as_str() {
        "codex" => {
            if cfg!(windows) {
                bail!(
                    "Codex setup is not available on Windows yet: the hook needs a POSIX shell. \
                     Codex sessions still run in Sakshi, without live state"
                );
            }
            let dir = sakshi_store::data_dir()
                .context("cannot find a data directory")?
                .join("codex-plugin");
            codex(&RealCodex, &dir, options, input, out)
        }
        "claude" => bail!("Claude Code needs no setup: Sakshi wires its hooks in for each session"),
        other => bail!("no setup for `{other}`; only `codex` has one"),
    }
}

fn describe(status: Status) -> String {
    if !status.enabled {
        "not set up".to_string()
    } else if status.is_ready() {
        format!("set up, all {} hooks approved", status.total)
    } else {
        format!(
            "installed, but only {} of {} hooks are approved in Codex (the rest are skipped)",
            status.trusted, status.total
        )
    }
}

fn codex(
    cli: &dyn CodexCli,
    dir: &Path,
    options: Options,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<()> {
    if !cli.available() {
        bail!("could not run `codex --version`; install Codex first");
    }
    let before = codex_plugin::status(&cli.config_text());

    if options.status {
        writeln!(out, "Codex: {}", describe(before))?;
        if !before.is_ready() {
            writeln!(out, "Run `sakshi setup codex` to fix that.")?;
        }
        return Ok(());
    }

    if options.remove {
        // Both steps are attempted; one already being gone is fine.
        let plugin = cli.run(&["plugin", "remove", codex_plugin::PLUGIN_ID]);
        let market = cli.run(&["plugin", "marketplace", "remove", codex_plugin::NAME]);
        if let (Err(a), Err(b)) = (&plugin, &market) {
            if before.enabled {
                bail!("could not remove the plugin: {a}; {b}");
            }
        }
        writeln!(
            out,
            "Removed Sakshi's Codex plugin. Codex is as it was before setup."
        )?;
        return Ok(());
    }

    if before.is_ready() {
        writeln!(out, "Codex: {}. Nothing to do.", describe(before))?;
        return Ok(());
    }

    if !options.yes {
        writeln!(
            out,
            "Codex only runs hooks you have approved, so Sakshi needs a one-time setup.\n\
             \n\
             This will run, through Codex's own CLI:\n  \
             codex plugin marketplace add {}\n  \
             codex plugin add {}\n\
             \n\
             That adds a small plugin to your Codex config. Its hooks do nothing unless Codex\n\
             is running inside Sakshi. Undo it any time with `sakshi setup codex --remove`.\n\
             \n\
             Continue? [y/N] ",
            dir.display(),
            codex_plugin::PLUGIN_ID
        )?;
        out.flush()?;
        let mut answer = String::new();
        input.read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            writeln!(out, "Nothing changed.")?;
            return Ok(());
        }
    }

    codex_plugin::write_files(dir).context("could not write the plugin files")?;
    if !before.enabled {
        let dir_text = dir.to_string_lossy();
        let market = cli.run(&["plugin", "marketplace", "add", &dir_text]);
        let plugin = cli.run(&["plugin", "add", codex_plugin::PLUGIN_ID]);
        match (market, plugin) {
            (_, Ok(())) => {}
            (Ok(()), Err(e)) | (Err(e), Err(_)) => return Err(e),
        }
    }

    let after = codex_plugin::status(&cli.config_text());
    writeln!(out, "Codex: {}.", describe(after))?;
    if !after.is_ready() {
        writeln!(
            out,
            "\nLast step, in Codex: start it (press `o` in Sakshi) and, when it says hooks\n\
             need review, choose \"Trust all and continue\". Check with `sakshi setup codex --status`."
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A Codex that records what it was asked and keeps a fake config.
    struct Fake {
        installed: bool,
        calls: RefCell<Vec<String>>,
        config: RefCell<String>,
        fail: Option<&'static str>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                installed: true,
                calls: RefCell::new(Vec::new()),
                config: RefCell::new(String::new()),
                fail: None,
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl CodexCli for Fake {
        fn available(&self) -> bool {
            self.installed
        }
        fn run(&self, args: &[&str]) -> Result<()> {
            let line = args.join(" ");
            self.calls.borrow_mut().push(line.clone());
            if self.fail.is_some_and(|f| line.contains(f)) {
                bail!("`codex {line}` failed: boom");
            }
            if line == "plugin add sakshi@sakshi" {
                *self.config.borrow_mut() = "[plugins.\"sakshi@sakshi\"]\nenabled = true\n".into();
            }
            Ok(())
        }
        fn config_text(&self) -> String {
            self.config.borrow().clone()
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sakshi-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn go(fake: &Fake, dir: &Path, options: Options, answer: &str) -> (Result<()>, String) {
        let mut out = Vec::new();
        let result = codex(fake, dir, options, &mut answer.as_bytes(), &mut out);
        (result, String::from_utf8_lossy(&out).into_owned())
    }

    fn approved_config() -> String {
        let mut text = String::from("[plugins.\"sakshi@sakshi\"]\nenabled = true\n[hooks.state]\n");
        for event in codex_plugin::TRUST_EVENTS {
            text.push_str(&format!(
                "[hooks.state.\"sakshi@sakshi:hooks/hooks.json:{event}:0:0\"]\ntrusted_hash = \"sha256:ab\"\n"
            ));
        }
        text
    }

    #[test]
    fn status_changes_nothing() {
        let fake = Fake::new();
        let dir = temp_dir("status");
        let (r, out) = go(
            &fake,
            &dir,
            Options {
                status: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_ok());
        assert!(out.contains("not set up"), "{out}");
        assert!(fake.calls().is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn declining_changes_nothing() {
        for answer in ["n\n", "\n", "", "maybe\n"] {
            let fake = Fake::new();
            let dir = temp_dir("decline");
            let (r, out) = go(&fake, &dir, Options::default(), answer);
            assert!(r.is_ok());
            assert!(out.contains("Nothing changed"), "{answer:?}: {out}");
            assert!(fake.calls().is_empty(), "{answer:?}");
            assert!(!dir.exists(), "no files before consent");
        }
    }

    #[test]
    fn consent_is_explained_before_it_is_asked() {
        let fake = Fake::new();
        let dir = temp_dir("explain");
        let (_, out) = go(&fake, &dir, Options::default(), "n\n");
        assert!(out.contains("codex plugin add sakshi@sakshi"), "{out}");
        assert!(out.contains("--remove"), "{out}");
    }

    #[test]
    fn saying_yes_installs_through_the_codex_cli_then_asks_for_approval() {
        let fake = Fake::new();
        let dir = temp_dir("install");
        let (r, out) = go(&fake, &dir, Options::default(), "y\n");
        assert!(r.is_ok(), "{r:?}");
        let calls = fake.calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].starts_with("plugin marketplace add "), "{calls:?}");
        assert!(
            calls[0].ends_with(dir.to_string_lossy().as_ref()),
            "{calls:?}"
        );
        assert_eq!(calls[1], "plugin add sakshi@sakshi");
        assert!(dir.join("plugins/sakshi/hooks/hooks.json").is_file());
        assert!(out.contains("Trust all and continue"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn yes_skips_the_question() {
        let fake = Fake::new();
        let dir = temp_dir("yes");
        let (r, out) = go(
            &fake,
            &dir,
            Options {
                yes: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_ok());
        assert!(!out.contains("Continue?"), "{out}");
        assert_eq!(fake.calls().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_set_up_and_approved_does_nothing() {
        let fake = Fake::new();
        *fake.config.borrow_mut() = approved_config();
        let dir = temp_dir("ready");
        let (r, out) = go(&fake, &dir, Options::default(), "y\n");
        assert!(r.is_ok());
        assert!(out.contains("Nothing to do"), "{out}");
        assert!(fake.calls().is_empty());
        assert!(!dir.exists());
    }

    #[test]
    fn installed_but_not_approved_is_not_reinstalled_but_still_guides_the_user() {
        let fake = Fake::new();
        *fake.config.borrow_mut() = "[plugins.\"sakshi@sakshi\"]\nenabled = true\n".into();
        let dir = temp_dir("unapproved");
        let (r, out) = go(
            &fake,
            &dir,
            Options {
                yes: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_ok());
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        assert!(out.contains("Trust all and continue"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_undoes_both_steps_and_tolerates_one_being_gone() {
        let mut fake = Fake::new();
        fake.fail = Some("marketplace remove");
        *fake.config.borrow_mut() = approved_config();
        let (r, out) = go(
            &fake,
            &temp_dir("remove"),
            Options {
                remove: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            fake.calls(),
            [
                "plugin remove sakshi@sakshi",
                "plugin marketplace remove sakshi"
            ]
        );
        assert!(out.contains("Removed"), "{out}");
    }

    #[test]
    fn remove_fails_loudly_when_an_installed_plugin_cannot_be_removed() {
        let mut fake = Fake::new();
        fake.fail = Some("remove");
        *fake.config.borrow_mut() = approved_config();
        let (r, _) = go(
            &fake,
            &temp_dir("remove-fail"),
            Options {
                remove: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_err());
    }

    #[test]
    fn a_codex_that_cannot_be_run_is_reported() {
        let mut fake = Fake::new();
        fake.installed = false;
        let (r, _) = go(
            &fake,
            &temp_dir("missing"),
            Options {
                yes: true,
                ..Options::default()
            },
            "",
        );
        let message = format!("{:#}", r.unwrap_err());
        assert!(message.contains("install Codex"), "{message}");
    }

    #[test]
    fn a_failing_plugin_add_is_an_error() {
        let mut fake = Fake::new();
        fake.fail = Some("plugin add");
        let dir = temp_dir("add-fails");
        let (r, _) = go(
            &fake,
            &dir,
            Options {
                yes: true,
                ..Options::default()
            },
            "",
        );
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_agents_are_explained() {
        let mut out = Vec::new();
        let claude = run("claude", Options::default(), &mut "".as_bytes(), &mut out);
        assert!(format!("{:#}", claude.unwrap_err()).contains("no setup"));
        let other = run("vim", Options::default(), &mut "".as_bytes(), &mut out);
        assert!(other.is_err());
    }
}
