//! Opt-in test against a real, logged-in `codex`. It makes one tiny request
//! through your own Codex account, so it never runs by default:
//!
//! ```sh
//! cargo test -p sakshi --test codex_real -- --ignored --nocapture
//! SAKSHI_TEST_BIN=/path/to/released/sakshi cargo test -p sakshi --test codex_real -- --ignored
//! ```
//!
//! It uses a throwaway `CODEX_HOME` (your login is linked in, nothing else is
//! shared), so your own Codex config is untouched.
//!
//! The case it exists for: Codex runs hooks inside a shared background server
//! that keeps the environment it first started with. A server already running
//! when Sakshi launches Codex used to swallow every hook, because they never
//! saw the session's `SAKSHI_*` variables. The unit tests cannot see that, so
//! this starts such a server first and then checks the events still arrive.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{sakshi_bin, DataDir, Tui};
use sakshi_core::{Event, EventKind, SessionState};

/// A scratch `CODEX_HOME` holding only a link to the real login. Stops any
/// background server started inside it when dropped.
struct CodexHome(PathBuf);

impl CodexHome {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sakshi-codex-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
            .expect("cannot find the real Codex home");
        let auth = real.join("auth.json");
        assert!(auth.exists(), "log in with `codex login` first: {auth:?}");
        std::os::unix::fs::symlink(auth, dir.join("auth.json")).unwrap();
        Self(dir)
    }

    /// Run `codex <args>` against this home, without any Sakshi variables.
    fn codex(&self, args: &[&str]) {
        let status = Command::new("codex")
            .args(args)
            .env("CODEX_HOME", &self.0)
            .env_remove("SAKSHI_ENDPOINT")
            .env_remove("SAKSHI_SESSION_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("codex must be installed");
        assert!(status.success(), "`codex {}` failed", args.join(" "));
    }
}

impl Drop for CodexHome {
    fn drop(&mut self) {
        let _ = Command::new("codex")
            .args(["app-server", "daemon", "stop"])
            .env("CODEX_HOME", &self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `PATH` with the directory of the `sakshi` under test first: the plugin's
/// hooks look `sakshi` up there.
fn path_with_sakshi() -> String {
    let dir = Path::new(&sakshi_bin()).parent().unwrap().to_path_buf();
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing}", dir.display())
}

/// Detach from the running session and quit, which kills it.
async fn quit(tui: &mut Tui) {
    tui.send(b"\x1d");
    tui.wait_for("the list", |s| s.contains("Enter attach"))
        .await;
    tui.send(b"q");
    tui.wait_for("the quit warning", |s| s.contains("Press q again"))
        .await;
    tui.send(b"q");
    assert!(tui.wait_for_exit().await, "sakshi should exit cleanly");
}

#[tokio::test]
#[ignore = "needs a logged-in codex and makes one real request; run with --ignored"]
async fn a_real_codex_reports_through_the_hooks_even_with_a_shared_server_running() {
    let data = DataDir::new("codex");
    let home = CodexHome::new("hooks");

    // A shared server that has never heard of this Sakshi session.
    home.codex(&["app-server", "daemon", "start"]);

    // The one-time setup, as a user would run it.
    let setup = Command::new(sakshi_bin())
        .args(["setup", "codex", "--yes"])
        .env("CODEX_HOME", &home.0)
        .env("SAKSHI_DATA_DIR", &data.0)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    // Answer Codex's questions through its config file, not `-c` flags or
    // `--dangerously-bypass-hook-trust`: any override makes Codex start a
    // server of its own, which hides exactly the shared-server path under test.
    let work = home.0.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let config_path = home.0.join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        format!(
            "check_for_update_on_startup = false\n{config}\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            work.display()
        ),
    )
    .unwrap();
    let work_dir = work.to_string_lossy().into_owned();
    let env = || {
        vec![
            (
                "CODEX_HOME".to_string(),
                home.0.to_string_lossy().into_owned(),
            ),
            ("PATH".to_string(), path_with_sakshi()),
        ]
    };

    // First launch: Codex asks to approve the plugin's hooks, as the README
    // tells users to expect. Approved hooks only run from the next launch, and
    // the approval is only saved once a turn has completed.
    let mut tui = Tui::start_with_env(
        &["--no-notify", "codex", "-C", &work_dir, "Say hello"],
        &data.0,
        env(),
    );
    tui.wait_for_within("the hook review", Duration::from_secs(120), |s| {
        s.contains("Hooks need review")
    })
    .await;
    // Codex draws this prompt a moment before it reads keys. Down to "Trust
    // all and continue", then confirm, as separate keypresses.
    tokio::time::sleep(Duration::from_secs(4)).await;
    tui.send(b"\x1b[B");
    tokio::time::sleep(Duration::from_secs(1)).await;
    tui.send(b"\r");
    tui.wait_for_within("the first turn", Duration::from_secs(240), |s| {
        s.contains("Worked for")
    })
    .await;
    quit(&mut tui).await;
    let saved = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        saved.contains("hooks.state"),
        "Codex did not save the hook approval:\n{saved}"
    );

    // Second launch, with the shared server still running from the first: a
    // task whose hooks must reach this session, not the first one's.
    let prompt = "Run the shell command `echo sakshi-real-test`, then say done";
    let mut tui = Tui::start_with_env(
        &["--no-notify", "codex", "-C", &work_dir, prompt],
        &data.0,
        env(),
    );
    // Codex prints "Worked for Ns" when the turn is over, then waits for more
    // input rather than exiting.
    tui.wait_for_within("the end of the turn", Duration::from_secs(240), |s| {
        s.contains("Worked for")
    })
    .await;
    quit(&mut tui).await;

    let logs = sakshi_store::list_logs(&sakshi_store::sessions_dir(&data.0)).unwrap();
    assert_eq!(logs.len(), 2, "{logs:?}");
    // Log file names start with the start time, so the last one is the task run.
    let task_log = logs.iter().max_by_key(|l| l.path.clone()).unwrap();
    let events: Vec<Event> = sakshi_store::read_events(&task_log.path).unwrap();
    eprintln!("recorded events:");
    for event in &events {
        eprintln!("  {:?}", event.kind);
    }
    let kinds: Vec<&EventKind> = events.iter().map(|e| &e.kind).collect();

    assert!(
        matches!(kinds.first(), Some(EventKind::Session { agent, .. }) if agent == "codex"),
        "the session should be recorded as a codex session"
    );
    // These can only have come from the real agent's hooks.
    assert!(
        kinds.contains(&&EventKind::State(SessionState::Working)),
        "no 'working' state arrived: the hooks did not reach this session"
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::Command { command } if command.contains("echo sakshi-real-test"))),
        "the command the agent ran was not recorded"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(&e.kind, EventKind::Trust { .. })
                && format!("{:?}", e.kind).contains("hooks-silent")),
        "the session was flagged hooks-silent"
    );
}
