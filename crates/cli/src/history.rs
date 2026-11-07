//! `sakshi sessions`, `sakshi report` and `sakshi replay`: read what past
//! sessions did from their activity logs.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use sakshi_core::{timeline_line, Event, Report};
use sakshi_store::{list_logs, read_events, LogInfo};
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;

/// Longest pause a replay will sit through between two events.
const MAX_REPLAY_PAUSE: Duration = Duration::from_millis(1500);

/// Output format of `sakshi report`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Markdown, for reading.
    Md,
    /// A short Markdown block for a pull request description.
    Pr,
    /// JSON, for scripts.
    Json,
}

/// Pick a log by `latest` or by a unique prefix of its session id.
pub fn resolve<'a>(logs: &'a [LogInfo], query: &str) -> Result<&'a LogInfo> {
    if logs.is_empty() {
        bail!("no recorded sessions yet: run sakshi first");
    }
    let query = query.trim().to_ascii_lowercase();
    if query == "latest" {
        return Ok(&logs[0]);
    }
    let matches: Vec<&LogInfo> = logs
        .iter()
        .filter(|l| !query.is_empty() && l.id.to_string().starts_with(&query))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("no session matches {query:?}: see `sakshi sessions`"),
        many => bail!(
            "{query:?} matches {} sessions; use more digits: {}",
            many.len(),
            many.iter()
                .map(|l| l.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn load(dir: &Path, query: &str) -> Result<(LogInfo, Vec<Event>)> {
    let logs = list_logs(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    let log = resolve(&logs, query)?.clone();
    let events =
        read_events(&log.path).with_context(|| format!("cannot read {}", log.path.display()))?;
    Ok((log, events))
}

/// Treat a closed pipe (`sakshi replay | head`) as a normal end.
fn finish(result: io::Result<()>) -> Result<()> {
    match result {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other.context("cannot write output"),
    }
}

/// Print a table of recorded sessions, newest first.
pub fn sessions(dir: &Path, limit: usize, out: &mut dyn Write) -> Result<()> {
    let logs = list_logs(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    if logs.is_empty() {
        return finish(writeln!(out, "No recorded sessions yet."));
    }
    let mut table = vec![format!(
        "{:<16}  {:<20}  {:<8}  {:<18}  {:<7}  {}",
        "SESSION", "STARTED", "AGENT", "NAME", "STATE", "TRUST"
    )];
    for log in logs.iter().take(limit) {
        let events = read_events(&log.path).unwrap_or_default();
        let started = log
            .started
            .format(&Rfc3339)
            .unwrap_or_else(|_| "?".to_string());
        let row = match Report::from_events(&events) {
            Some(r) => format!(
                "{:<16}  {:<20}  {:<8}  {:<18}  {:<7}  {}",
                log.id,
                started,
                r.agent,
                r.name,
                r.final_state.map_or("-", |s| s.as_str()),
                r.trust.badge
            ),
            None => format!("{:<16}  {:<20}  (empty)", log.id, started),
        };
        table.push(row);
    }
    let hidden = logs.len().saturating_sub(limit);
    if hidden > 0 {
        table.push(format!("... and {hidden} older (use --limit)"));
    }
    finish(table.iter().try_for_each(|line| writeln!(out, "{line}")))
}

/// Print a report on one session.
pub fn report(dir: &Path, query: &str, format: Format, out: &mut dyn Write) -> Result<()> {
    let (log, events) = load(dir, query)?;
    let Some(report) = Report::from_events(&events) else {
        bail!("{} has no events", log.path.display());
    };
    let text = match format {
        Format::Md => report.to_markdown(),
        Format::Pr => report.to_pr_markdown(),
        Format::Json => {
            let mut text = serde_json::to_string_pretty(&report_json(&report))
                .context("cannot render JSON")?;
            text.push('\n');
            text
        }
    };
    finish(out.write_all(text.as_bytes()))
}

fn report_json(r: &Report) -> Value {
    let stamp = |t: time::OffsetDateTime| t.format(&Rfc3339).unwrap_or_default();
    json!({
        "session_id": r.session_id.to_string(),
        "name": r.name,
        "agent": r.agent,
        "cwd": r.cwd,
        "started": stamp(r.started),
        "ended": stamp(r.ended),
        "duration_seconds": r.duration_seconds(),
        "final_state": r.final_state.map(|s| s.as_str()),
        "event_count": r.event_count,
        "trust": {
            "badge": r.trust.badge.as_str(),
            "findings": r.trust.findings.iter().map(|f| json!({
                "rule": f.rule,
                "severity": f.severity.as_str(),
                "reason": f.reason,
            })).collect::<Vec<_>>(),
        },
        "summary": {
            "tool_uses": r.summary.tool_uses,
            "files_edited": r.summary.files_edited,
            "commands": r.summary.commands,
            "tests": {
                "passed": r.summary.tests_passed,
                "failed": r.summary.tests_failed,
                "unclear": r.summary.tests_unknown,
            },
            "last_message": r.summary.last_message,
        },
    })
}

/// Pause to use between two events when replaying at `speed` times real time.
/// `None` means do not wait at all.
pub fn replay_pause(gap_seconds: f64, speed: f64) -> Option<Duration> {
    if speed <= 0.0 || !speed.is_finite() || gap_seconds <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(gap_seconds / speed).min(MAX_REPLAY_PAUSE))
}

/// Print a session's timeline, optionally paced like the original.
pub async fn replay(dir: &Path, query: &str, speed: f64, out: &mut dyn Write) -> Result<()> {
    let (log, events) = load(dir, query)?;
    let Some(first) = events.first() else {
        bail!("{} has no events", log.path.display());
    };
    let start = first.ts;
    let mut previous = start;
    for event in &events {
        let gap = (event.ts - previous).as_seconds_f64();
        if let Some(pause) = replay_pause(gap, speed) {
            tokio::time::sleep(pause).await;
        }
        previous = event.ts;
        let line = timeline_line(event, start);
        if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
            return finish(Err(e));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sakshi_core::SessionId;
    use time::OffsetDateTime;

    use super::*;

    fn log(id: u64, secs: i64) -> LogInfo {
        LogInfo {
            id: SessionId(id),
            path: format!("{id}.jsonl").into(),
            started: OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).unwrap(),
        }
    }

    #[test]
    fn latest_is_the_first_log() {
        let logs = [log(0xaa, 10), log(0xbb, 0)];
        assert_eq!(resolve(&logs, "latest").unwrap().id, SessionId(0xaa));
        assert_eq!(resolve(&logs, " LATEST ").unwrap().id, SessionId(0xaa));
    }

    #[test]
    fn a_unique_prefix_selects_a_session() {
        let logs = [log(0xaa00, 10), log(0xbb00, 0)];
        // 0xaa00 renders as 000000000000aa00.
        assert_eq!(
            resolve(&logs, "000000000000aa").unwrap().id,
            SessionId(0xaa00)
        );
        assert_eq!(
            resolve(&logs, "000000000000BB00").unwrap().id,
            SessionId(0xbb00)
        );
    }

    #[test]
    fn ambiguous_unknown_and_empty_queries_are_errors() {
        let logs = [log(0xaa00, 10), log(0xaa01, 0)];
        let ambiguous = resolve(&logs, "000000000000aa").unwrap_err().to_string();
        assert!(ambiguous.contains("matches 2 sessions"), "{ambiguous}");
        assert!(resolve(&logs, "ffff")
            .unwrap_err()
            .to_string()
            .contains("no session matches"));
        assert!(resolve(&logs, "").is_err());
        assert!(resolve(&[], "latest")
            .unwrap_err()
            .to_string()
            .contains("no recorded sessions"));
    }

    #[test]
    fn replay_pacing_scales_and_caps() {
        assert_eq!(replay_pause(10.0, 0.0), None);
        assert_eq!(replay_pause(10.0, f64::NAN), None);
        assert_eq!(replay_pause(0.0, 10.0), None);
        assert_eq!(replay_pause(-3.0, 10.0), None);
        assert_eq!(replay_pause(10.0, 10.0), Some(Duration::from_secs(1)));
        assert_eq!(replay_pause(3600.0, 2.0), Some(MAX_REPLAY_PAUSE));
    }
}
