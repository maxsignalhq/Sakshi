# Future enhancements

Ideas that fit Sakshi's job (watch, never act) and keep it light. Nothing here is
promised; each is small enough to be one pull request. Anything that would make
Sakshi orchestrate, decide or phone home is out.

## Shipped in 0.4.0

`secret-added` and `large-change` (see [TRUST.md](../TRUST.md)).

## Dropped

- **Session cost or token totals.** The hook payloads Sakshi receives carry no
  usage data, so this would mean parsing the agent's private transcript files,
  which break whenever the agent changes their format. Not worth the weight. Revisit
  if an agent starts reporting usage in its hooks.

## Considered and left out

- Anything that merges, removes or fixes things for you (Sakshi is a witness).
- Orchestration or agent-to-agent messaging (that is a framework).
- Team dashboards or any network feature (local-first).
- A third agent adapter (on request only).
