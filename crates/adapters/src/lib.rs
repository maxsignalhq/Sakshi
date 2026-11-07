//! Agent adapters: one per supported coding agent.
//!
//! An adapter knows how to recognise its agent's command line, check that the
//! agent is installed, wire the agent's hooks to Sakshi, and translate the
//! agent's hook payloads into Sakshi events.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod claude;
mod codex;
pub mod codex_plugin;

use std::io;
use std::path::Path;

use sakshi_core::{AgentKind, Event, SessionId};

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;

/// Everything an adapter needs to prepare a launch.
#[derive(Debug, Clone, Copy)]
pub struct LaunchContext<'a> {
    /// Path of the running `sakshi` executable, which hooks call back into.
    pub hook_exe: &'a Path,
    /// Private directory where adapters may write generated config files.
    pub runtime_dir: &'a Path,
    /// The program being launched, exactly as the user gave it.
    pub program: &'a str,
    /// The arguments the user passed, before any the adapter adds.
    pub user_args: &'a [String],
}

/// Support for one coding agent.
pub trait AgentAdapter: Send + Sync {
    /// Which agent this adapter handles.
    fn kind(&self) -> AgentKind;

    /// Name shown to the user, e.g. `Claude Code`.
    fn display_name(&self) -> &'static str;

    /// Program to run when the user asks for this agent without a command.
    fn default_program(&self) -> &'static str;

    /// Key that starts this agent from the session list. Defaults to the
    /// first letter of the program; override it when that is taken.
    fn start_key(&self) -> char {
        self.default_program()
            .chars()
            .next()
            .map_or('?', |c| c.to_ascii_lowercase())
    }

    /// Whether `program`, as typed on the command line, is this agent.
    fn matches(&self, program: &str) -> bool;

    /// Whether the agent is installed and runnable. May spawn a process.
    fn probe(&self) -> bool;

    /// Arguments to put before the user's own so the agent reports to Sakshi.
    fn launch_args(&self, ctx: &LaunchContext<'_>) -> io::Result<Vec<String>>;

    /// Translate one hook payload (the JSON the agent gives a hook on stdin)
    /// into events for `session_id`. Unknown payloads yield no events.
    fn translate_hook(&self, session_id: SessionId, payload: &serde_json::Value) -> Vec<Event>;
}

/// Every adapter Sakshi knows, whether or not the agent is installed.
pub fn all() -> Vec<Box<dyn AgentAdapter>> {
    vec![Box::new(ClaudeAdapter), Box::new(CodexAdapter)]
}

/// The adapter for `program`, if any.
pub fn for_program(program: &str) -> Option<Box<dyn AgentAdapter>> {
    all().into_iter().find(|a| a.matches(program))
}

/// The adapter whose hook name is `name` (the argument of `sakshi hook <name>`).
pub fn for_hook_name(name: &str) -> Option<Box<dyn AgentAdapter>> {
    all()
        .into_iter()
        .find(|a| a.default_program().eq_ignore_ascii_case(name))
}

/// Adapters whose agent is installed. Agents that are not installed are hidden.
pub fn available() -> Vec<Box<dyn AgentAdapter>> {
    all().into_iter().filter(|a| a.probe()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_adapter_by_program_name() {
        assert!(for_program("claude").is_some());
        assert!(for_program("/usr/local/bin/claude").is_some());
        assert!(for_program("bash").is_none());
    }

    #[test]
    fn finds_the_codex_adapter() {
        assert!(for_program("codex").is_some());
        assert!(for_program("/opt/homebrew/bin/codex").is_some());
        assert!(for_hook_name("codex").is_some());
    }

    #[test]
    fn start_keys_are_unique() {
        let keys: Vec<char> = all().iter().map(|a| a.start_key()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "{keys:?}");
    }

    #[test]
    fn finds_adapter_by_hook_name() {
        assert!(for_hook_name("claude").is_some());
        assert!(for_hook_name("Claude").is_some());
        assert!(for_hook_name("nope").is_none());
    }
}
