//! Best-effort removal of secrets from text before it is written to disk.
//!
//! Commands an agent runs can carry credentials: `curl -H "Authorization:
//! Bearer ..."`, `--token ...`, `PASSWORD=... make deploy`, a pasted API key.
//! Sakshi records commands in its activity log, so it masks the obvious
//! shapes first. This is a safety net, not a guarantee: a secret in an
//! unusual shape will get through.

use crate::event::{Event, EventKind};
use crate::trust::Finding;

/// What a masked secret is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Words after which the next word is a secret.
const SECRET_FLAGS: &[&str] = &[
    "bearer",
    "--token",
    "--password",
    "--passwd",
    "--secret",
    "--api-key",
    "--apikey",
    "--auth-token",
    "--access-token",
    "--client-secret",
];

/// Substrings of a `name=value` name that mark the value as secret.
const SECRET_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "credential",
    "private_key",
    "private-key",
];

/// Well-known credential prefixes and the shortest length of a real one.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("github_pat_", 20),
    ("glpat-", 16),
    ("sk-ant-", 16),
    ("sk-", 20),
    ("xoxb-", 16),
    ("xoxp-", 16),
    ("npm_", 20),
    ("AIza", 30),
];

/// Mask secrets in `text`, leaving everything else (including whitespace)
/// exactly as it was.
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_word = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let is_space = rest.starts_with(char::is_whitespace);
        let end = rest
            .find(|c: char| c.is_whitespace() != is_space)
            .unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(end);
        if is_space {
            out.push_str(chunk);
        } else {
            let after_flag = SECRET_FLAGS.contains(&previous_word.as_str());
            out.push_str(&redact_word(chunk, after_flag));
            previous_word = chunk
                .trim_matches(|c: char| matches!(c, '"' | '\'' | ':'))
                .to_ascii_lowercase();
        }
        rest = tail;
    }
    out
}

/// Mask one whitespace-free word. `after_flag` says the word before it was a
/// flag that takes a secret.
fn redact_word(word: &str, after_flag: bool) -> String {
    let (open, core, close) = split_quotes(word);
    if after_flag && !core.is_empty() {
        return format!("{open}{REDACTED}{close}");
    }
    if let Some((name, _value)) = core.split_once('=') {
        let lower = name.to_ascii_lowercase();
        if SECRET_NAMES.iter().any(|n| lower.contains(n)) {
            return format!("{open}{name}={REDACTED}{close}");
        }
    }
    if is_known_token(core) {
        return format!("{open}{REDACTED}{close}");
    }
    if let Some(masked) = mask_url_password(core) {
        return format!("{open}{masked}{close}");
    }
    word.to_string()
}

/// Split surrounding quotes and trailing punctuation off a word.
fn split_quotes(word: &str) -> (&str, &str, &str) {
    let start = word
        .find(|c: char| !matches!(c, '"' | '\''))
        .unwrap_or(word.len());
    let trimmed_end = word[start..]
        .rfind(|c: char| !matches!(c, '"' | '\'' | ';' | ',' | ')'))
        .map_or(start, |i| {
            start + i + word[start + i..].chars().next().map_or(0, char::len_utf8)
        });
    (
        &word[..start],
        &word[start..trimmed_end],
        &word[trimmed_end..],
    )
}

fn is_known_token(word: &str) -> bool {
    let long_enough = |prefix: &str, min: usize| word.starts_with(prefix) && word.len() >= min;
    if TOKEN_PREFIXES.iter().any(|(p, min)| long_enough(p, *min)) {
        return true;
    }
    // AWS access key ids: AKIA or ASIA plus 16 upper-case letters or digits.
    word.len() == 20
        && (word.starts_with("AKIA") || word.starts_with("ASIA"))
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// `scheme://user:password@host` with the password masked.
fn mask_url_password(word: &str) -> Option<String> {
    let scheme_end = word.find("://")? + 3;
    let at = word[scheme_end..].find('@')? + scheme_end;
    let userinfo = &word[scheme_end..at];
    // A `/` before the `@` means the `@` belongs to the path, not userinfo.
    if userinfo.contains('/') {
        return None;
    }
    let (user, _password) = userinfo.split_once(':')?;
    Some(format!(
        "{}{user}:{REDACTED}{}",
        &word[..scheme_end],
        &word[at..]
    ))
}

/// A copy of `event` that is safe to write to disk.
pub fn redact_event(event: &Event) -> Event {
    let kind = match &event.kind {
        EventKind::Command { command } => EventKind::Command {
            command: redact_secrets(command),
        },
        EventKind::TestRun { command, passed } => EventKind::TestRun {
            command: redact_secrets(command),
            passed: *passed,
        },
        EventKind::Message { text } => EventKind::Message {
            text: redact_secrets(text),
        },
        EventKind::ToolUse { tool, summary } => EventKind::ToolUse {
            tool: tool.clone(),
            summary: summary.as_deref().map(redact_secrets),
        },
        EventKind::Trust { badge, findings } => EventKind::Trust {
            badge: *badge,
            findings: findings
                .iter()
                .map(|f| Finding {
                    rule: f.rule.clone(),
                    severity: f.severity,
                    reason: redact_secrets(&f.reason),
                })
                .collect(),
        },
        other => other.clone(),
    };
    Event {
        session_id: event.session_id,
        ts: event.ts,
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    fn masked(text: &str) -> String {
        redact_secrets(text)
    }

    #[test]
    fn ordinary_text_is_untouched_including_whitespace() {
        for text in [
            "",
            "cargo test -p sakshi-core",
            "ls  -la\n\tsrc",
            "git commit -m \"fix the token parser\"",
            "mkdir -p a/b && echo hello",
            "export PATH=/usr/bin:$PATH",
            "basic usage of sk",
        ] {
            assert_eq!(masked(text), text);
        }
    }

    #[test]
    fn bearer_tokens_and_secret_flags_are_masked() {
        assert_eq!(
            masked("curl -H \"Authorization: Bearer abc123def\" https://x"),
            "curl -H \"Authorization: Bearer [redacted]\" https://x"
        );
        assert_eq!(
            masked("deploy --token s3cr3t --env prod"),
            "deploy --token [redacted] --env prod"
        );
        assert_eq!(
            masked("login --password hunter2"),
            "login --password [redacted]"
        );
    }

    #[test]
    fn secret_named_assignments_are_masked() {
        assert_eq!(
            masked("PASSWORD=hunter2 make deploy"),
            "PASSWORD=[redacted] make deploy"
        );
        assert_eq!(
            masked("--client-secret=abc --name=ok"),
            "--client-secret=[redacted] --name=ok"
        );
        assert_eq!(
            masked("export AWS_SECRET_ACCESS_KEY=abcd1234"),
            "export AWS_SECRET_ACCESS_KEY=[redacted]"
        );
    }

    #[test]
    fn well_known_token_shapes_are_masked() {
        for token in [
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_11ABCDEFG0123456789_abcdef",
            "sk-ant-api03-abcdefghijklmnop",
            "sk-abcdefghijklmnopqrstuvwxyz",
            "xoxb-1234567890-abcdefgh",
            "AKIAABCDEFGHIJKLMNOP",
            "glpat-abcdefghijklmnop",
        ] {
            assert_eq!(
                masked(&format!("run {token} now")),
                "run [redacted] now",
                "{token}"
            );
        }
    }

    #[test]
    fn short_lookalikes_are_left_alone() {
        for word in ["sk-", "sk-short", "ghp_", "AKIA123", "npm_x"] {
            assert_eq!(masked(word), word);
        }
    }

    #[test]
    fn url_passwords_are_masked_but_plain_urls_are_not() {
        assert_eq!(
            masked("git clone https://bob:pa55@github.com/x/y.git"),
            "git clone https://bob:[redacted]@github.com/x/y.git"
        );
        assert_eq!(
            masked("curl https://example.com/a@b"),
            "curl https://example.com/a@b"
        );
        assert_eq!(
            masked("git clone https://github.com/x/y.git"),
            "git clone https://github.com/x/y.git"
        );
    }

    #[test]
    fn events_are_redacted_field_by_field() {
        let secret_cmd = "curl --token abc123 https://x";
        let ev = |kind| redact_event(&Event::now(SessionId(1), kind));
        let command = ev(EventKind::Command {
            command: secret_cmd.into(),
        });
        assert_eq!(
            command.kind,
            EventKind::Command {
                command: "curl --token [redacted] https://x".into()
            }
        );
        let edit = EventKind::FileEdit {
            path: "src/token.rs".into(),
        };
        assert_eq!(ev(edit.clone()).kind, edit);
        let message = ev(EventKind::Message {
            text: "set PASSWORD=x first".into(),
        });
        assert_eq!(
            message.kind,
            EventKind::Message {
                text: "set PASSWORD=[redacted] first".into()
            }
        );
    }
}
