//! `sakshi hook <agent>`: the shim agents call from their hooks.
//!
//! Fails open. Whatever goes wrong, the shim prints nothing and exits 0, so an
//! agent is never blocked or confused by Sakshi being absent, busy or broken.

use std::io::Read;
use std::time::Duration;

use sakshi_core::{Event, SessionId};
use sakshi_ipc::{send_events, Endpoint, ENDPOINT_ENV, SESSION_ENV};

/// Largest hook payload read from stdin.
const MAX_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024;

/// How long the shim waits on Sakshi before giving up.
const SEND_TIMEOUT: Duration = Duration::from_millis(500);

/// Handle one hook invocation. Never fails.
pub async fn run(agent: &str) {
    let Some(session) = std::env::var(SESSION_ENV).ok() else {
        return; // Not started by Sakshi: nothing to report to.
    };
    let Some(endpoint) = std::env::var(ENDPOINT_ENV).ok().map(Endpoint::from_raw) else {
        return;
    };
    let mut payload = String::new();
    if std::io::stdin()
        .take(MAX_PAYLOAD_BYTES)
        .read_to_string(&mut payload)
        .is_err()
    {
        return;
    }
    let Some(events) = events_for(agent, &session, &payload) else {
        return;
    };
    // The only failure possible here is Sakshi being unreachable; ignore it.
    let _ = send_events(&endpoint, &events, SEND_TIMEOUT).await;
}

/// Events for a hook payload, or `None` if there is nothing to send.
fn events_for(agent: &str, session: &str, payload: &str) -> Option<Vec<Event>> {
    let session_id = session.parse::<SessionId>().ok()?;
    let adapter = sakshi_adapters::for_hook_name(agent)?;
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let events = adapter.translate_hook(session_id, &value);
    (!events.is_empty()).then_some(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakshi_core::{EventKind, SessionState};

    const PROMPT: &str = r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;

    #[test]
    fn translates_a_valid_payload() {
        let events = events_for("claude", "00000000000000aa", PROMPT).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, SessionId(0xaa));
        assert_eq!(events[0].kind, EventKind::State(SessionState::Working));
    }

    #[test]
    fn rejects_bad_inputs_quietly() {
        assert!(events_for("claude", "not-hex", PROMPT).is_none());
        assert!(events_for("unknown-agent", "aa", PROMPT).is_none());
        assert!(events_for("claude", "aa", "not json").is_none());
        assert!(events_for("claude", "aa", "").is_none());
        assert!(events_for("claude", "aa", r#"{"hook_event_name":"PreCompact"}"#).is_none());
    }
}
