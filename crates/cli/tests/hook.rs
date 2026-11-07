//! End-to-end tests of the `sakshi hook` shim, run as the real binary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use sakshi_core::{EventKind, SessionId, SessionState};
use sakshi_ipc::{Endpoint, IpcServer, RuntimeDir, ENDPOINT_ENV, SESSION_ENV};

fn temp_runtime(tag: &str) -> RuntimeDir {
    RuntimeDir::create_unique(&std::env::temp_dir(), &format!("sakshi-hooktest-{tag}")).unwrap()
}

/// Run `sakshi hook claude` with `stdin` and the given Sakshi environment.
fn run_hook(stdin: &str, session: Option<&str>, endpoint: Option<&Endpoint>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sakshi"));
    cmd.args(["hook", "claude"])
        .env_remove(SESSION_ENV)
        .env_remove(ENDPOINT_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(s) = session {
        cmd.env(SESSION_ENV, s);
    }
    if let Some(e) = endpoint {
        cmd.env(ENDPOINT_ENV, e.as_str());
    }
    let mut child = cmd.spawn().unwrap();
    // A hook that has nothing to report may exit without reading its input, so
    // a broken pipe here is fine.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

const PROMPT: &str = r#"{"hook_event_name":"UserPromptSubmit","prompt":"go"}"#;

fn assert_silent_success(out: &Output) {
    assert!(out.status.success(), "exit: {:?}", out.status);
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
}

#[tokio::test]
async fn hook_delivers_events_to_a_running_server() {
    let dir = temp_runtime("deliver");
    let endpoint = dir.endpoint();
    let mut server = IpcServer::start(&endpoint).unwrap();

    let ep = endpoint.clone();
    let out =
        tokio::task::spawn_blocking(move || run_hook(PROMPT, Some("00000000000000ab"), Some(&ep)))
            .await
            .unwrap();
    assert_silent_success(&out);

    let ev = tokio::time::timeout(Duration::from_secs(5), server.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ev.session_id, SessionId(0xab));
    assert_eq!(ev.kind, EventKind::State(SessionState::Working));
}

#[test]
fn hook_outside_sakshi_does_nothing() {
    let out = run_hook(PROMPT, None, None);
    assert_silent_success(&out);
}

#[test]
fn hook_with_no_listening_server_fails_open_and_fast() {
    let dir = temp_runtime("absent");
    let started = Instant::now();
    let out = run_hook(PROMPT, Some("00000000000000ab"), Some(&dir.endpoint()));
    assert_silent_success(&out);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn hook_with_garbage_input_fails_open() {
    let dir = temp_runtime("garbage");
    for input in ["", "not json", "[1,2,3]", "{\"hook_event_name\":42}"] {
        let out = run_hook(input, Some("00000000000000ab"), Some(&dir.endpoint()));
        assert_silent_success(&out);
    }
}

#[test]
fn hook_with_bad_session_id_fails_open() {
    let dir = temp_runtime("badid");
    let out = run_hook(PROMPT, Some("not-an-id"), Some(&dir.endpoint()));
    assert_silent_success(&out);
}
