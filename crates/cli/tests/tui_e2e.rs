//! End-to-end tests of the real TUI: run `sakshi` inside a PTY, type at it, and
//! read its screen back through a terminal emulator.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{DataDir, Tui, CTRL_RIGHT_BRACKET, SHELL};

#[tokio::test]
async fn runs_a_shell_detaches_starts_a_second_session_and_quits() {
    let data = DataDir::new("main");
    let mut tui = Tui::start(&["--no-notify", SHELL], &data.0);

    // The first session starts attached, beside the session list.
    tui.wait_for("the session list", |s| {
        s.contains("Sessions") && s.contains("1 ")
    })
    .await;
    tui.wait_for("the attached footer", |s| s.contains("Ctrl+] detach"))
        .await;

    // Typing reaches the shell and its output comes back.
    tui.send(b"echo sakshi-e2e-ok\r");
    tui.wait_for("the echoed output", |s| {
        s.matches("sakshi-e2e-ok").count() >= 2
    })
    .await;

    // Ctrl+] detaches to the list.
    tui.send(&[CTRL_RIGHT_BRACKET]);
    tui.wait_for("the list footer", |s| s.contains("Enter attach"))
        .await;

    // `t` toggles the details panel with the trust verdict.
    tui.send(b"t");
    tui.wait_for("the details panel", |s| {
        s.contains("Details") && s.contains("Trust  GREEN")
    })
    .await;

    // `n` starts a second session and attaches to it.
    tui.send(b"n");
    tui.wait_for("a second session", |s| {
        s.contains("2 ") && s.contains("Ctrl+] detach")
    })
    .await;

    // Detach again; quitting with running sessions asks first.
    tui.send(&[CTRL_RIGHT_BRACKET]);
    tui.wait_for("the list footer again", |s| s.contains("Enter attach"))
        .await;
    tui.send(b"q");
    tui.wait_for("the quit confirmation", |s| s.contains("q again"))
        .await;
    tui.send(b"q");

    assert!(tui.wait_for_exit().await, "sakshi should exit cleanly");

    // One activity log per session, each starting with the session record.
    let logs = sakshi_store::list_logs(&sakshi_store::sessions_dir(&data.0)).unwrap();
    assert_eq!(logs.len(), 2, "one log per session: {logs:?}");
    for log in &logs {
        let events = sakshi_store::read_events(&log.path).unwrap();
        assert!(
            matches!(
                events.first().map(|e| &e.kind),
                Some(sakshi_core::EventKind::Session { .. })
            ),
            "{}: {events:?}",
            log.path.display()
        );
    }
}

#[tokio::test]
async fn a_multi_line_paste_reaches_the_shell_line_by_line() {
    let data = DataDir::new("paste");
    let mut tui = Tui::start(&["--no-notify", SHELL], &data.0);
    tui.wait_for("the attached footer", |s| s.contains("Ctrl+] detach"))
        .await;

    // Unix terminals mark a paste; the Windows console sends plain key presses
    // and Sakshi recognises the burst.
    #[cfg(not(windows))]
    {
        // Sakshi must have asked the terminal to mark pastes.
        assert!(
            tui.raw().windows(8).any(|w| w == b"\x1b[?2004h"),
            "bracketed paste was never enabled"
        );
        tui.send(b"\x1b[200~echo paste-one\necho paste-two\n\x1b[201~");
    }
    #[cfg(windows)]
    tui.send(b"echo paste-one\recho paste-two\r");

    // Each line ran: its text shows once as typed and once as output.
    tui.wait_for("both pasted lines to run", |s| {
        s.matches("paste-one").count() >= 2 && s.matches("paste-two").count() >= 2
    })
    .await;
}
