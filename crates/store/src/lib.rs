//! Sakshi's on-disk data: the per-session activity log.
//!
//! Each session appends to its own JSONL file. A line is exactly one IPC
//! protocol message, so a log can be replayed or fed back through any reader
//! of the protocol. Nothing here touches the network.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use sakshi_core::{Event, SessionId};
use sakshi_ipc::{encode_line, parse_line};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

/// Environment variable that overrides where Sakshi keeps its data.
pub const DATA_DIR_ENV: &str = "SAKSHI_DATA_DIR";

/// Sakshi's data directory, or `None` if the platform has no home directory.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    ProjectDirs::from("dev", "maxsignalhq", "sakshi").map(|d| d.data_local_dir().to_path_buf())
}

/// Environment variable that points at a specific config file.
pub const CONFIG_ENV: &str = "SAKSHI_CONFIG";

/// Where Sakshi looks for its optional `config.toml`, or `None` if the
/// platform has no home directory. `SAKSHI_CONFIG` overrides it.
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    ProjectDirs::from("dev", "maxsignalhq", "sakshi").map(|d| d.config_dir().join("config.toml"))
}

/// Where session logs live inside a data directory.
pub fn sessions_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

/// `20261024T101500Z`: sorts chronologically and is safe in file names.
fn format_stamp(ts: OffsetDateTime) -> String {
    let ts = ts.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        ts.year(),
        u8::from(ts.month()),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
}

fn parse_stamp(stamp: &str) -> Option<OffsetDateTime> {
    let b = stamp.as_bytes();
    if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<u32>().ok();
    let date = Date::from_calendar_date(
        i32::try_from(num(0..4)?).ok()?,
        Month::try_from(u8::try_from(num(4..6)?).ok()?).ok()?,
        u8::try_from(num(6..8)?).ok()?,
    )
    .ok()?;
    let time = Time::from_hms(
        u8::try_from(num(9..11)?).ok()?,
        u8::try_from(num(11..13)?).ok()?,
        u8::try_from(num(13..15)?).ok()?,
    )
    .ok()?;
    Some(PrimitiveDateTime::new(date, time).assume_utc())
}

/// An open, append-only log for one session.
pub struct ActivityLog {
    file: File,
    path: PathBuf,
}

impl ActivityLog {
    /// Create `<dir>/<start>-<session id>.jsonl`, creating `dir` if needed.
    pub fn create(dir: &Path, id: SessionId, started: OffsetDateTime) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}-{id}.jsonl", format_stamp(started)));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { file, path })
    }

    /// Append one event. Flushed immediately so a crash loses nothing.
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        let mut line = encode_line(event);
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()
    }

    /// The log file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Writes events into one log per session, opening logs as sessions appear.
///
/// Failures are logged and swallowed: losing the activity log must never
/// disturb a running session.
pub struct LogSet {
    dir: PathBuf,
    logs: HashMap<SessionId, ActivityLog>,
}

impl LogSet {
    /// Write logs into `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            logs: HashMap::new(),
        }
    }

    /// Append `event` to its session's log.
    pub fn record(&mut self, event: &Event) {
        if !self.logs.contains_key(&event.session_id) {
            match ActivityLog::create(&self.dir, event.session_id, event.ts) {
                Ok(log) => {
                    self.logs.insert(event.session_id, log);
                }
                Err(e) => {
                    tracing::warn!("cannot open activity log in {}: {e}", self.dir.display());
                    return;
                }
            }
        }
        if let Some(log) = self.logs.get_mut(&event.session_id) {
            if let Err(e) = log.append(event) {
                tracing::warn!("cannot write {}: {e}", log.path().display());
            }
        }
    }
}

/// A log file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogInfo {
    /// The session the log belongs to.
    pub id: SessionId,
    /// The log file.
    pub path: PathBuf,
    /// When the session started, from the file name.
    pub started: OffsetDateTime,
}

/// Parse `<stamp>-<id>.jsonl`.
fn parse_log_name(path: &Path) -> Option<LogInfo> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".jsonl")?;
    let (stamp, id) = stem.split_once('-')?;
    Some(LogInfo {
        id: id.parse().ok()?,
        started: parse_stamp(stamp)?,
        path: path.to_path_buf(),
    })
}

/// The logs in `dir`, newest first. A missing directory means no logs.
pub fn list_logs(dir: &Path) -> io::Result<Vec<LogInfo>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut logs: Vec<LogInfo> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| parse_log_name(&entry.path()))
        .collect();
    logs.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| b.id.cmp(&a.id)));
    Ok(logs)
}

/// Read every event in a log. Lines that cannot be parsed are skipped, so a
/// log damaged by a crash or written by a newer Sakshi still loads.
pub fn read_events(path: &Path) -> io::Result<Vec<Event>> {
    let reader = BufReader::new(File::open(path)?);
    let mut events = Vec::new();
    for line in reader.split(b'\n') {
        let line = line?;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        match parse_line(text) {
            Ok(event) => events.push(event),
            Err(e) => tracing::debug!("skipping unreadable log line in {}: {e}", path.display()),
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use sakshi_core::{Badge, EventKind, SessionState};

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sakshi-store-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn at(secs: i64, id: u64, kind: EventKind) -> Event {
        Event {
            session_id: SessionId(id),
            ts: OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap(),
            kind,
        }
    }

    fn msg(secs: i64, id: u64, text: &str) -> Event {
        at(secs, id, EventKind::Message { text: text.into() })
    }

    #[test]
    fn stamps_round_trip() {
        let ts = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let stamp = format_stamp(ts);
        assert_eq!(stamp.len(), 16);
        assert_eq!(parse_stamp(&stamp), Some(ts));
    }

    #[test]
    fn bad_stamps_are_rejected() {
        for s in [
            "",
            "2026",
            "20261324T000000Z",
            "20260101T000000",
            "garbage-garbage1",
        ] {
            assert_eq!(parse_stamp(s), None, "{s}");
        }
    }

    #[test]
    fn events_survive_a_write_and_read() {
        let dir = Scratch::new("roundtrip");
        let events = vec![
            at(
                0,
                7,
                EventKind::Session {
                    name: "claude".into(),
                    agent: "claude".into(),
                    cwd: "/w".into(),
                },
            ),
            at(1, 7, EventKind::State(SessionState::Working)),
            at(
                2,
                7,
                EventKind::Trust {
                    badge: Badge::Green,
                    findings: Vec::new(),
                },
            ),
            msg(3, 7, "multi\nline \"quoted\""),
        ];
        let mut log = ActivityLog::create(&dir.0, SessionId(7), events[0].ts).unwrap();
        for e in &events {
            log.append(e).unwrap();
        }
        assert_eq!(read_events(log.path()).unwrap(), events);
    }

    #[test]
    fn appending_continues_an_existing_log() {
        let dir = Scratch::new("append");
        let first = msg(0, 1, "a");
        let mut log = ActivityLog::create(&dir.0, SessionId(1), first.ts).unwrap();
        log.append(&first).unwrap();
        let path = log.path().to_path_buf();
        drop(log);
        let mut again = ActivityLog::create(&dir.0, SessionId(1), first.ts).unwrap();
        again.append(&msg(1, 1, "b")).unwrap();
        assert_eq!(read_events(&path).unwrap().len(), 2);
    }

    #[test]
    fn unreadable_lines_are_skipped() {
        let dir = Scratch::new("damaged");
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("log.jsonl");
        let good = encode_line(&msg(0, 1, "kept"));
        let mut content = b"not json\n\xff\xfe\n".to_vec();
        content.extend_from_slice(good.as_bytes());
        content.extend_from_slice(b"\n{\"v\":1,\"truncated\n");
        std::fs::write(&path, content).unwrap();
        let events = read_events(&path).unwrap();
        assert_eq!(events, vec![msg(0, 1, "kept")]);
    }

    #[test]
    fn a_log_set_keeps_one_file_per_session() {
        let dir = Scratch::new("logset");
        let mut set = LogSet::new(dir.0.clone());
        set.record(&msg(0, 1, "a1"));
        set.record(&msg(1, 2, "b1"));
        set.record(&msg(2, 1, "a2"));
        let logs = list_logs(&dir.0).unwrap();
        assert_eq!(logs.len(), 2);
        let counts: HashMap<SessionId, usize> = logs
            .iter()
            .map(|l| (l.id, read_events(&l.path).unwrap().len()))
            .collect();
        assert_eq!(counts[&SessionId(1)], 2);
        assert_eq!(counts[&SessionId(2)], 1);
    }

    #[test]
    fn logs_are_listed_newest_first_and_junk_is_ignored() {
        let dir = Scratch::new("list");
        let mut set = LogSet::new(dir.0.clone());
        set.record(&msg(0, 1, "old"));
        set.record(&msg(100, 2, "new"));
        std::fs::write(dir.0.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.0.join("bad-name.jsonl"), "x").unwrap();
        let logs = list_logs(&dir.0).unwrap();
        let ids: Vec<_> = logs.iter().map(|l| l.id).collect();
        assert_eq!(ids, [SessionId(2), SessionId(1)]);
    }

    #[test]
    fn a_missing_directory_has_no_logs() {
        let dir = Scratch::new("missing");
        assert!(list_logs(&dir.0).unwrap().is_empty());
    }

    #[test]
    fn the_data_dir_can_be_overridden() {
        // Only this test touches the variable, so setting it here is safe.
        std::env::set_var(DATA_DIR_ENV, "/somewhere/else");
        assert_eq!(data_dir(), Some(PathBuf::from("/somewhere/else")));
        std::env::remove_var(DATA_DIR_ENV);
        assert_ne!(data_dir(), Some(PathBuf::from("/somewhere/else")));
    }
}
