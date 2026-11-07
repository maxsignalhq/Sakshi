# Future enhancements

Ideas that fit Sakshi's job (watch, never act) and keep it light. Nothing here is
promised; each is small enough to be one pull request. Anything that would make
Sakshi orchestrate, decide or phone home is out.

## Planned for 0.4.0

- **`secret-added` rule (alert).** An added diff line that looks like a real
  credential (the key shapes `core::redact` already knows). Agents hardcoding keys
  is a common, costly slip. The finding names the file, never the secret.
- **`large-change` rule (info, warn when huge).** A session that changed many
  files or lines. The point is to tell a reviewer "this is a big one" before they
  open the diff. Thresholds are constants, adjustable in `config.toml` only if
  people ask.
- **Session cost in the report, only if cheap.** If the agent's hook payloads
  already carry token usage, show a total in the session summary and the report.
  If getting it needs scraping the screen or reading the agent's private files, drop
  it.

## Considered and left out

- Anything that merges, removes or fixes things for you (Sakshi is a witness).
- Orchestration or agent-to-agent messaging (that is a framework).
- Team dashboards or any network feature (local-first).
- A third agent adapter (on request only).
