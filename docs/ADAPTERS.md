# Writing an adapter

An adapter teaches Sakshi about one coding agent, such as Codex, Gemini CLI,
Aider or OpenCode. Claude Code is the reference implementation:
[`crates/adapters/src/claude.rs`](../crates/adapters/src/claude.rs).

You can get value without writing Rust: anything that can write a line of JSON
to a socket can report to Sakshi. See [Without an adapter](#without-an-adapter).

## What an adapter does

1. **Recognises** the agent's command line (`codex`, `/usr/local/bin/codex`).
2. **Probes** whether the agent is installed, so its key only shows when it works.
3. **Wires up hooks** at launch, by returning extra arguments (usually a generated
   config file) that make the agent call `sakshi hook <name>` on its events.
4. **Translates** each hook payload, the JSON the agent hands its hook on stdin,
   into Sakshi events: state changes, tool use, file edits, commands, test runs.

Everything after that, including live state, notifications, trust rules, logs and
reports, works for free.

## Steps

1. **Add a variant** to `AgentKind` in `crates/core/src/session.rs`, with its
   name in `as_str`. Any kind other than `Generic` is expected to report through
   hooks, so the `hooks-silent` trust rule covers your agent automatically.
2. **Implement `AgentAdapter`** in a new `crates/adapters/src/<agent>.rs`:

   ```rust
   pub struct CodexAdapter;

   impl AgentAdapter for CodexAdapter {
       fn kind(&self) -> AgentKind { AgentKind::Codex }
       fn display_name(&self) -> &'static str { "Codex" }
       fn default_program(&self) -> &'static str { "codex" }      // also the hook name
       fn matches(&self, program: &str) -> bool { /* compare the file stem */ }
       fn probe(&self) -> bool { /* run `codex --version` quietly */ }
       fn launch_args(&self, ctx: &LaunchContext<'_>) -> io::Result<Vec<String>> {
           // Write a config file into ctx.runtime_dir whose hooks run
           // `<ctx.hook_exe> hook codex`, then return the flag that loads it.
       }
       fn translate_hook(&self, id: SessionId, payload: &Value) -> Vec<Event> {
           // Map the agent's hook JSON to events. Unknown payloads: return vec![].
       }
   }
   ```

3. **Register it** in `all()` in `crates/adapters/src/lib.rs`.
4. **Pick a key** for starting it from the session list (`crates/tui`), and show it
   only when `probe()` succeeds.
5. **Test it** the way `claude.rs` does: feed recorded hook payloads to
   `translate_hook` and assert on the events. Real payloads beat invented ones;
   paste a few from the agent's docs or a debug log into the tests.
6. **Document it**: a section in [PROTOCOL.md](../PROTOCOL.md) listing which hooks
   map to which events, and the agent added to the README's supported list.

## Rules

- **Fail open.** If Sakshi is not running, the agent must behave exactly as if
  Sakshi did not exist. `sakshi hook` already prints nothing, exits 0 and gives up
  after 500 ms; keep the generated config from adding anything that could block.
- **Never touch the user's own config.** Generate a file in `ctx.runtime_dir` and
  pass it with a flag. If the agent can only read config from a fixed location,
  open an issue before writing code so we can find another way.
- **Report what you know, skip what you don't.** A `test_run` with an unknown
  outcome is better than a guessed one. Use `sakshi_core::is_test_command` to
  recognise test runners.
- **Map states carefully.** `waiting` means the user is needed (a question, a
  permission prompt, a finished turn); that is what triggers notifications.
- **Cross-platform.** Paths in generated config must work on Windows too.

## Without an adapter

Any program started inside Sakshi gets two environment variables,
`SAKSHI_ENDPOINT` and `SAKSHI_SESSION_ID`. Send newline-delimited JSON to that
endpoint as described in [PROTOCOL.md](../PROTOCOL.md), for example from an
agent's hook script or your own wrapper. This is also a quick way to prototype an
adapter before writing Rust.
