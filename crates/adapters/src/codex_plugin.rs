//! The Codex plugin that carries Sakshi's hooks.
//!
//! Codex only runs hooks the user has approved, and it records that approval
//! against a hash of the plugin's files. So, unlike Claude Code, hooks cannot be
//! injected for one session. Instead `sakshi setup codex` registers this one
//! plugin, once, through Codex's own CLI, and the user approves it in Codex.
//!
//! **These files are frozen.** Changing a single byte of the manifest or the
//! hooks (even the description) makes Codex ask every user to approve them
//! again, and until they do the hooks are skipped without a warning. A change
//! here needs a deliberate release note. `frozen_files_have_not_changed` is the
//! tripwire.
//!
//! The hook does nothing unless `SAKSHI_ENDPOINT` is set, which only Sakshi
//! does, so Codex sessions outside Sakshi are unaffected. It always exits 0.

use std::io;
use std::path::{Path, PathBuf};

/// Plugin and marketplace name, so the plugin id is `sakshi@sakshi`.
pub const NAME: &str = "sakshi";

/// The id Codex uses for the plugin in its config.
pub const PLUGIN_ID: &str = "sakshi@sakshi";

/// Events the plugin subscribes to, as Codex's config names them in trust keys.
pub const TRUST_EVENTS: &[&str] = &[
    "session_start",
    "session_end",
    "user_prompt_submit",
    "pre_tool_use",
    "permission_request",
    "post_tool_use",
    "stop",
    "interrupt",
];

/// `.agents/plugins/marketplace.json`: a local marketplace holding one plugin.
pub const MARKETPLACE_JSON: &str = r#"{
  "name": "sakshi",
  "interface": { "displayName": "Sakshi" },
  "plugins": [
    {
      "name": "sakshi",
      "source": { "source": "local", "path": "./plugins/sakshi" },
      "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" },
      "category": "Developer Tools"
    }
  ]
}
"#;

/// `.codex-plugin/plugin.json`.
pub const PLUGIN_JSON: &str = r#"{
  "name": "sakshi",
  "version": "1.0.0",
  "description": "Reports Codex activity to a running Sakshi. Does nothing outside Sakshi.",
  "hooks": "./hooks/hooks.json"
}
"#;

/// `hooks/hooks.json`. Every hook runs the same guarded command: call
/// `sakshi hook codex` only inside a Sakshi session and only if `sakshi` is on
/// the path, otherwise swallow the payload. It always exits 0.
pub const HOOKS_JSON: &str = r#"{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "PreToolUse": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "PermissionRequest": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "PostToolUse": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }],
    "Interrupt": [{ "hooks": [{ "type": "command", "timeout": 3, "command": "sh -c 'if [ -n \"$SAKSHI_ENDPOINT\" ] && command -v sakshi >/dev/null 2>&1; then sakshi hook codex; else cat >/dev/null; fi; exit 0'" }] }]
  }
}
"#;

/// Write the marketplace (and the plugin inside it) under `dir`.
///
/// Writing identical bytes again is harmless, so setup can be re-run.
pub fn write_files(dir: &Path) -> io::Result<()> {
    let files: [(PathBuf, &str); 3] = [
        (
            dir.join(".agents").join("plugins").join("marketplace.json"),
            MARKETPLACE_JSON,
        ),
        (
            dir.join("plugins")
                .join(NAME)
                .join(".codex-plugin")
                .join("plugin.json"),
            PLUGIN_JSON,
        ),
        (
            dir.join("plugins")
                .join(NAME)
                .join("hooks")
                .join("hooks.json"),
            HOOKS_JSON,
        ),
    ];
    for (path, contents) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// What a Codex `config.toml` says about the Sakshi plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The plugin is installed and enabled.
    pub enabled: bool,
    /// How many of its hooks the user has approved.
    pub trusted: usize,
    /// How many hooks the plugin has.
    pub total: usize,
}

impl Status {
    /// Installed, enabled and every hook approved: Codex will report to Sakshi.
    pub fn is_ready(&self) -> bool {
        self.enabled && self.trusted == self.total
    }
}

/// Read the plugin's state out of the text of a Codex `config.toml`.
///
/// A missing or unreadable config means "not set up", never an error: this is
/// only used to tell the user what to do next.
pub fn status(config_toml: &str) -> Status {
    let total = TRUST_EVENTS.len();
    let Ok(config) = config_toml.parse::<toml::Table>() else {
        return Status {
            enabled: false,
            trusted: 0,
            total,
        };
    };
    let enabled = config
        .get("plugins")
        .and_then(|p| p.get(PLUGIN_ID))
        .and_then(|p| p.get("enabled"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let state = config
        .get("hooks")
        .and_then(|h| h.get("state"))
        .and_then(toml::Value::as_table);
    let trusted = TRUST_EVENTS
        .iter()
        .filter(|event| {
            let key = format!("{PLUGIN_ID}:hooks/hooks.json:{event}:0:0");
            state
                .and_then(|s| s.get(&key))
                .and_then(|entry| entry.get("trusted_hash"))
                .is_some()
        })
        .count();
    Status {
        enabled,
        trusted,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn frozen_files_have_not_changed() {
        // A byte count and a checksum-ish fingerprint of each file. If this
        // fails you changed a frozen file: every user must approve the hooks
        // again. Do that only on purpose, note it in the changelog, then update
        // these numbers.
        fn fingerprint(s: &str) -> (usize, u64) {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in s.bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            (s.len(), h)
        }
        assert_eq!(fingerprint(MARKETPLACE_JSON), EXPECTED_MARKETPLACE);
        assert_eq!(fingerprint(PLUGIN_JSON), EXPECTED_PLUGIN);
        assert_eq!(fingerprint(HOOKS_JSON), EXPECTED_HOOKS);
    }

    const EXPECTED_MARKETPLACE: (usize, u64) = (307, 0xb56566db3ef70cba);
    const EXPECTED_PLUGIN: (usize, u64) = (171, 0x6bd268e4ab6388d7);
    const EXPECTED_HOOKS: (usize, u64) = (1781, 0x399c02fa5ebf6b15);

    #[test]
    fn the_json_files_parse() {
        for text in [MARKETPLACE_JSON, PLUGIN_JSON, HOOKS_JSON] {
            assert!(serde_json::from_str::<Value>(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn every_event_is_wired_to_the_guarded_command_and_times_out() {
        let hooks: Value = serde_json::from_str(HOOKS_JSON).unwrap();
        let hooks = hooks["hooks"].as_object().unwrap();
        let names: Vec<&str> = hooks.keys().map(String::as_str).collect();
        assert_eq!(names.len(), TRUST_EVENTS.len());
        for (event, groups) in hooks {
            let hook = &groups[0]["hooks"][0];
            let command = hook["command"].as_str().unwrap();
            // Runs Sakshi only inside a Sakshi session, drains stdin otherwise,
            // and can never fail the agent's tool call.
            assert!(command.contains("[ -n \"$SAKSHI_ENDPOINT\" ]"), "{event}");
            assert!(command.contains("sakshi hook codex"), "{event}");
            assert!(command.contains("cat >/dev/null"), "{event}");
            assert!(command.ends_with("exit 0'"), "{event}");
            // No absolute path: a path would change when Sakshi is upgraded.
            assert!(
                !command.contains("/Users/") && !command.contains("/opt/"),
                "{event}"
            );
            assert_eq!(hook["timeout"], 3, "{event}");
        }
    }

    #[test]
    fn the_manifest_points_at_the_hooks_file() {
        let plugin: Value = serde_json::from_str(PLUGIN_JSON).unwrap();
        assert_eq!(plugin["name"], NAME);
        assert_eq!(plugin["hooks"], "./hooks/hooks.json");
        let market: Value = serde_json::from_str(MARKETPLACE_JSON).unwrap();
        assert_eq!(market["name"], NAME);
        assert_eq!(market["plugins"][0]["name"], NAME);
    }

    #[test]
    fn write_files_lays_out_a_marketplace_and_can_be_repeated() {
        let dir = std::env::temp_dir().join(format!("sakshi-plugin-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_files(&dir).unwrap();
        write_files(&dir).unwrap();
        let hooks = dir.join("plugins/sakshi/hooks/hooks.json");
        assert_eq!(std::fs::read_to_string(hooks).unwrap(), HOOKS_JSON);
        assert!(dir.join(".agents/plugins/marketplace.json").is_file());
        assert!(dir
            .join("plugins/sakshi/.codex-plugin/plugin.json")
            .is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn config_with(enabled: bool, trusted: &[&str]) -> String {
        let mut text = String::from("[plugins.\"sakshi@sakshi\"]\n");
        text.push_str(&format!("enabled = {enabled}\n[hooks.state]\n"));
        for event in trusted {
            text.push_str(&format!(
                "[hooks.state.\"sakshi@sakshi:hooks/hooks.json:{event}:0:0\"]\ntrusted_hash = \"sha256:ab\"\n"
            ));
        }
        text
    }

    #[test]
    fn status_reports_not_set_up_for_missing_or_broken_config() {
        for text in ["", "not toml [[[", "[model]\nname = \"x\"\n"] {
            let s = status(text);
            assert!(!s.enabled && s.trusted == 0 && !s.is_ready(), "{text:?}");
        }
    }

    #[test]
    fn status_counts_approved_hooks() {
        let partial = status(&config_with(true, &["pre_tool_use", "stop"]));
        assert!(partial.enabled);
        assert_eq!((partial.trusted, partial.total), (2, 8));
        assert!(!partial.is_ready());

        let full = status(&config_with(true, TRUST_EVENTS));
        assert!(full.is_ready());
    }

    #[test]
    fn status_ignores_other_plugins_and_a_disabled_plugin() {
        let other = "[plugins.\"other@market\"]\nenabled = true\n[hooks.state.\"other@market:hooks/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:ab\"\n";
        let s = status(other);
        assert!(!s.enabled && s.trusted == 0);
        assert!(!status(&config_with(false, TRUST_EVENTS)).is_ready());
    }
}
