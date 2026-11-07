//! End-to-end tests of `sakshi sessions`, `report` and `replay`, run as the
//! real binary against logs written with the real store.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::{Command, Output};

use sakshi_core::{Event, EventKind, SessionId, SessionState};
use sakshi_store::{sessions_dir, LogSet};
use time::OffsetDateTime;

struct DataDir(PathBuf);

impl DataDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sakshi-history-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// Write a session whose events start `age_secs` seconds after a fixed epoch.
    fn write_session(&self, id: u64, start: i64, name: &str, risky: bool) {
        let at = |secs: i64, kind| Event {
            session_id: SessionId(id),
            ts: OffsetDateTime::from_unix_timestamp(1_800_000_000 + start + secs).unwrap(),
            kind,
        };
        let mut events = vec![
            at(
                0,
                EventKind::Session {
                    name: name.into(),
                    agent: "claude".into(),
                    cwd: "/work/app".into(),
                },
            ),
            at(1, EventKind::State(SessionState::Working)),
            at(
                2,
                EventKind::FileEdit {
                    path: "src/main.rs".into(),
                },
            ),
        ];
        if risky {
            events.push(at(
                3,
                EventKind::Command {
                    command: "git push --force".into(),
                },
            ));
        }
        events.push(at(4, EventKind::State(SessionState::Done)));
        let mut logs = LogSet::new(sessions_dir(&self.0));
        for e in &events {
            logs.record(e);
        }
    }

    fn sakshi(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sakshi"))
            .args(args)
            .env("SAKSHI_DATA_DIR", &self.0)
            .output()
            .unwrap()
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.sakshi(args);
        assert!(
            out.status.success(),
            "sakshi {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sessions_lists_newest_first_with_state_and_trust() {
    let data = DataDir::new("list");
    data.write_session(0x1, 0, "older", false);
    data.write_session(0x2, 100, "newer", true);
    let out = data.stdout(&["sessions"]);
    assert!(out.contains("SESSION"), "{out}");
    let newer = out.find("newer").unwrap();
    let older = out.find("older").unwrap();
    assert!(newer < older, "{out}");
    let newer_row = out.lines().find(|l| l.contains("newer")).unwrap();
    assert!(
        newer_row.contains("done") && newer_row.contains("red"),
        "{newer_row}"
    );
    let older_row = out.lines().find(|l| l.contains("older")).unwrap();
    assert!(older_row.contains("green"), "{older_row}");
}

#[test]
fn sessions_respects_the_limit() {
    let data = DataDir::new("limit");
    for i in 0..3 {
        data.write_session(i + 1, i as i64 * 10, &format!("s{i}"), false);
    }
    let out = data.stdout(&["sessions", "--limit", "1"]);
    assert!(out.contains("and 2 older"), "{out}");
}

#[test]
fn an_empty_data_dir_says_so() {
    let data = DataDir::new("empty");
    assert!(data.stdout(&["sessions"]).contains("No recorded sessions"));
    let out = data.sakshi(&["report"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no recorded sessions"));
}

#[test]
fn report_renders_markdown_for_the_latest_session() {
    let data = DataDir::new("md");
    data.write_session(0x1, 0, "old", false);
    data.write_session(0x2, 100, "new", true);
    let md = data.stdout(&["report"]);
    for expected in [
        "# Sakshi report: new",
        "## Trust: RED",
        "destructive-command",
        "src/main.rs",
        "git push --force",
    ] {
        assert!(md.contains(expected), "missing {expected:?} in:\n{md}");
    }
    assert!(!md.contains("Sakshi report: old"));
}

#[test]
fn report_pr_format_is_a_short_pull_request_block() {
    let data = DataDir::new("pr");
    data.write_session(0x2, 100, "new", true);
    let md = data.stdout(&["report", "--format", "pr"]);
    assert!(
        md.starts_with("### Agent changes (recorded by Sakshi)"),
        "{md}"
    );
    assert!(
        md.contains("**Trust: RED**") && md.contains("src/main.rs"),
        "{md}"
    );
    assert!(!md.contains("Session:"), "{md}");
}

#[test]
fn report_can_pick_a_session_by_id_prefix() {
    let data = DataDir::new("prefix");
    data.write_session(0xabc1, 0, "first", false);
    data.write_session(0xdef2, 100, "second", false);
    let md = data.stdout(&["report", "000000000000abc"]);
    assert!(md.contains("Sakshi report: first"), "{md}");
    let bad = data.sakshi(&["report", "ffff"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("no session matches"));
}

#[test]
fn report_json_is_valid_and_complete() {
    let data = DataDir::new("json");
    data.write_session(0x2, 100, "new", true);
    let text = data.stdout(&["report", "latest", "--format", "json"]);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["name"], "new");
    assert_eq!(v["agent"], "claude");
    assert_eq!(v["final_state"], "done");
    assert_eq!(v["trust"]["badge"], "red");
    assert_eq!(v["trust"]["findings"][0]["rule"], "destructive-command");
    assert_eq!(v["summary"]["files_edited"][0], "src/main.rs");
    assert_eq!(v["duration_seconds"], 4);
}

#[test]
fn replay_prints_the_timeline_in_order() {
    let data = DataDir::new("replay");
    data.write_session(0x2, 100, "new", true);
    let out = data.stdout(&["replay"]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 5, "{out}");
    assert!(lines[0].starts_with("+00:00:00") && lines[0].contains("session started: new"));
    assert!(lines[2].contains("edit src/main.rs"), "{out}");
    assert!(lines[3].contains("$ git push --force"), "{out}");
    assert!(
        lines[4].starts_with("+00:00:04") && lines[4].contains("state: done"),
        "{out}"
    );
}

#[test]
fn replay_at_speed_still_finishes() {
    let data = DataDir::new("speed");
    data.write_session(0x2, 100, "new", false);
    let out = data.stdout(&["replay", "latest", "--speed", "1000"]);
    assert_eq!(out.lines().count(), 4, "{out}");
}
