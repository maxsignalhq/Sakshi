//! The Sakshi IPC protocol, version 1: one JSON object per line.
//!
//! ```json
//! {"v":1,"session_id":"...","type":"state","ts":"2026-01-01T00:00:00Z","data":{"state":"working"}}
//! ```
//!
//! See `PROTOCOL.md` for the full description.

use sakshi_core::{Badge, Event, EventKind, Finding, SessionId, SessionState, Severity};
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest accepted line, in bytes. Longer lines end the connection.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Why a line could not be turned into an [`Event`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The line is not a JSON object of the expected shape.
    #[error("invalid message: {0}")]
    Invalid(String),
    /// The message declares a protocol version this crate does not speak.
    #[error("unsupported protocol version {0}")]
    Version(u64),
    /// `session_id` is not a valid session id.
    #[error("invalid session id {0:?}")]
    SessionId(String),
    /// `ts` is not an RFC 3339 timestamp.
    #[error("invalid timestamp {0:?}")]
    Timestamp(String),
    /// `type` is not a known event type.
    #[error("unknown event type {0:?}")]
    UnknownType(String),
    /// A required `data` field is missing or has the wrong type.
    #[error("{ty} event: missing or invalid field {field:?}")]
    Field {
        /// The event type being decoded.
        ty: String,
        /// The offending field.
        field: &'static str,
    },
}

/// Parse one protocol line into an [`Event`]. Unknown extra fields are ignored.
pub fn parse_line(line: &str) -> Result<Event, ProtocolError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| ProtocolError::Invalid(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| ProtocolError::Invalid("not a JSON object".into()))?;

    let version = obj
        .get("v")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtocolError::Invalid("missing \"v\"".into()))?;
    if version != u64::from(PROTOCOL_VERSION) {
        return Err(ProtocolError::Version(version));
    }

    let session_text = text_field(obj, "session_id")?;
    let session_id = session_text
        .parse::<SessionId>()
        .map_err(|_| ProtocolError::SessionId(session_text.to_string()))?;

    let ts_text = text_field(obj, "ts")?;
    let ts = OffsetDateTime::parse(ts_text, &Rfc3339)
        .map_err(|_| ProtocolError::Timestamp(ts_text.to_string()))?;

    let ty = text_field(obj, "type")?;
    let data = obj.get("data").unwrap_or(&Value::Null);
    let kind = decode_kind(ty, data)?;

    Ok(Event {
        session_id,
        ts,
        kind,
    })
}

/// Encode `event` as one protocol line, without the trailing newline.
pub fn encode_line(event: &Event) -> String {
    let ts = event.ts.format(&Rfc3339).unwrap_or_default();
    json!({
        "v": PROTOCOL_VERSION,
        "session_id": event.session_id.to_string(),
        "type": event.kind.type_name(),
        "ts": ts,
        "data": encode_data(&event.kind),
    })
    .to_string()
}

fn text_field<'a>(
    obj: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, ProtocolError> {
    obj.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError::Invalid(format!("missing or non-string {field:?}")))
}

fn data_str(ty: &str, data: &Value, field: &'static str) -> Result<String, ProtocolError> {
    data.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ProtocolError::Field {
            ty: ty.to_string(),
            field,
        })
}

fn data_opt_str(data: &Value, field: &str) -> Option<String> {
    data.get(field).and_then(Value::as_str).map(str::to_string)
}

fn decode_kind(ty: &str, data: &Value) -> Result<EventKind, ProtocolError> {
    match ty {
        "state" => {
            let state = data_str(ty, data, "state")?;
            state
                .parse::<SessionState>()
                .map(EventKind::State)
                .map_err(|_| ProtocolError::Field {
                    ty: ty.to_string(),
                    field: "state",
                })
        }
        "tool_use" => Ok(EventKind::ToolUse {
            tool: data_str(ty, data, "tool")?,
            summary: data_opt_str(data, "summary"),
        }),
        "file_edit" => Ok(EventKind::FileEdit {
            path: data_str(ty, data, "path")?,
        }),
        "command" => Ok(EventKind::Command {
            command: data_str(ty, data, "command")?,
        }),
        "test_run" => Ok(EventKind::TestRun {
            command: data_str(ty, data, "command")?,
            passed: data.get("passed").and_then(Value::as_bool),
        }),
        "message" => Ok(EventKind::Message {
            text: data_str(ty, data, "text")?,
        }),
        "session" => Ok(EventKind::Session {
            name: data_str(ty, data, "name")?,
            agent: data_str(ty, data, "agent")?,
            cwd: data_str(ty, data, "cwd")?,
        }),
        "trust" => decode_trust(ty, data),
        other => Err(ProtocolError::UnknownType(other.to_string())),
    }
}

fn decode_trust(ty: &str, data: &Value) -> Result<EventKind, ProtocolError> {
    let bad = |field: &'static str| ProtocolError::Field {
        ty: ty.to_string(),
        field,
    };
    let badge = data_str(ty, data, "badge")?
        .parse::<Badge>()
        .map_err(|_| bad("badge"))?;
    let items = data
        .get("findings")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("findings"))?;
    let findings = items
        .iter()
        .map(|item| {
            Ok(Finding {
                rule: data_str(ty, item, "rule")?,
                severity: data_str(ty, item, "severity")?
                    .parse::<Severity>()
                    .map_err(|_| bad("severity"))?,
                reason: data_str(ty, item, "reason")?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolError>>()?;
    Ok(EventKind::Trust { badge, findings })
}

fn encode_data(kind: &EventKind) -> Value {
    match kind {
        EventKind::State(state) => json!({ "state": state.as_str() }),
        EventKind::ToolUse { tool, summary } => json!({ "tool": tool, "summary": summary }),
        EventKind::FileEdit { path } => json!({ "path": path }),
        EventKind::Command { command } => json!({ "command": command }),
        EventKind::TestRun { command, passed } => json!({ "command": command, "passed": passed }),
        EventKind::Message { text } => json!({ "text": text }),
        EventKind::Session { name, agent, cwd } => {
            json!({ "name": name, "agent": agent, "cwd": cwd })
        }
        EventKind::Trust { badge, findings } => json!({
            "badge": badge.as_str(),
            "findings": findings
                .iter()
                .map(|f| json!({
                    "rule": f.rule,
                    "severity": f.severity.as_str(),
                    "reason": f.reason,
                }))
                .collect::<Vec<_>>(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ty: &str, data: &str) -> String {
        format!(
            r#"{{"v":1,"session_id":"00000000000000ff","type":"{ty}","ts":"2026-03-04T05:06:07Z","data":{data}}}"#
        )
    }

    #[test]
    fn parses_every_event_type() {
        let cases = [
            ("state", r#"{"state":"waiting"}"#, "state"),
            ("tool_use", r#"{"tool":"Bash"}"#, "tool_use"),
            ("file_edit", r#"{"path":"src/a.rs"}"#, "file_edit"),
            ("command", r#"{"command":"ls"}"#, "command"),
            (
                "test_run",
                r#"{"command":"cargo test","passed":true}"#,
                "test_run",
            ),
            ("message", r#"{"text":"hi"}"#, "message"),
            (
                "session",
                r#"{"name":"n","agent":"claude","cwd":"/x"}"#,
                "session",
            ),
            ("trust", r#"{"badge":"yellow","findings":[]}"#, "trust"),
        ];
        for (ty, data, name) in cases {
            let ev = parse_line(&line(ty, data)).unwrap();
            assert_eq!(ev.session_id, SessionId(255));
            assert_eq!(ev.kind.type_name(), name);
        }
    }

    #[test]
    fn decodes_field_values() {
        let ev = parse_line(&line("state", r#"{"state":"waiting"}"#)).unwrap();
        assert_eq!(ev.kind, EventKind::State(SessionState::Waiting));
        let ev = parse_line(&line("test_run", r#"{"command":"pytest"}"#)).unwrap();
        assert_eq!(
            ev.kind,
            EventKind::TestRun {
                command: "pytest".into(),
                passed: None
            }
        );
        assert_eq!(ev.ts.year(), 2026);
    }

    #[test]
    fn round_trips_through_encode() {
        let kinds = [
            EventKind::State(SessionState::Working),
            EventKind::ToolUse {
                tool: "Edit".into(),
                summary: Some("a.rs".into()),
            },
            EventKind::ToolUse {
                tool: "Read".into(),
                summary: None,
            },
            EventKind::FileEdit { path: "p".into() },
            EventKind::Command {
                command: "ls \"x\"".into(),
            },
            EventKind::TestRun {
                command: "t".into(),
                passed: Some(false),
            },
            EventKind::Message {
                text: "multi\nline".into(),
            },
        ];
        for kind in kinds {
            let ev = Event::now(SessionId(7), kind);
            let encoded = encode_line(&ev);
            assert!(!encoded.contains('\n'));
            let decoded = parse_line(&encoded).unwrap();
            assert_eq!(decoded.session_id, ev.session_id);
            assert_eq!(decoded.kind, ev.kind);
            // RFC 3339 keeps sub-second precision.
            assert_eq!(decoded.ts, ev.ts);
        }
    }

    #[test]
    fn ignores_unknown_fields() {
        let l = r#"{"v":1,"session_id":"1","type":"message","ts":"2026-01-01T00:00:00Z","data":{"text":"x","extra":1},"future":true}"#;
        assert!(parse_line(l).is_ok());
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(matches!(
            parse_line("not json"),
            Err(ProtocolError::Invalid(_))
        ));
        assert!(matches!(parse_line("[1]"), Err(ProtocolError::Invalid(_))));
        assert!(matches!(parse_line("{}"), Err(ProtocolError::Invalid(_))));
        assert!(matches!(parse_line(""), Err(ProtocolError::Invalid(_))));
    }

    #[test]
    fn rejects_bad_version_session_timestamp_and_type() {
        let base = |v: &str, s: &str, t: &str, ty: &str| {
            format!(
                r#"{{"v":{v},"session_id":"{s}","type":"{ty}","ts":"{t}","data":{{"text":"x"}}}}"#
            )
        };
        assert_eq!(
            parse_line(&base("2", "1", "2026-01-01T00:00:00Z", "message")),
            Err(ProtocolError::Version(2))
        );
        assert!(matches!(
            parse_line(&base("1", "zz", "2026-01-01T00:00:00Z", "message")),
            Err(ProtocolError::SessionId(_))
        ));
        assert!(matches!(
            parse_line(&base("1", "1", "yesterday", "message")),
            Err(ProtocolError::Timestamp(_))
        ));
        assert!(matches!(
            parse_line(&base("1", "1", "2026-01-01T00:00:00Z", "dance")),
            Err(ProtocolError::UnknownType(_))
        ));
    }

    #[test]
    fn rejects_missing_or_mistyped_data_fields() {
        assert!(matches!(
            parse_line(&line("file_edit", "{}")),
            Err(ProtocolError::Field { field: "path", .. })
        ));
        assert!(matches!(
            parse_line(&line("command", r#"{"command":5}"#)),
            Err(ProtocolError::Field {
                field: "command",
                ..
            })
        ));
        assert!(matches!(
            parse_line(&line("state", r#"{"state":"napping"}"#)),
            Err(ProtocolError::Field { field: "state", .. })
        ));
        assert!(matches!(
            parse_line(&line("trust", r#"{"badge":"purple","findings":[]}"#)),
            Err(ProtocolError::Field { field: "badge", .. })
        ));
        assert!(matches!(
            parse_line(&line("trust", r#"{"badge":"red"}"#)),
            Err(ProtocolError::Field {
                field: "findings",
                ..
            })
        ));
        assert!(matches!(
            parse_line(&line(
                "trust",
                r#"{"badge":"red","findings":[{"rule":"r","severity":"dire","reason":"x"}]}"#
            )),
            Err(ProtocolError::Field {
                field: "severity",
                ..
            })
        ));
        // A missing `data` object behaves like an empty one.
        let l = r#"{"v":1,"session_id":"1","type":"message","ts":"2026-01-01T00:00:00Z"}"#;
        assert!(matches!(parse_line(l), Err(ProtocolError::Field { .. })));
    }
}
