# Future enhancements

Ideas are measured against the test in [SPEC.md](../SPEC.md#deciding-what-gets-in):
does it make the answer to "can I trust what it just did?" more accurate, more
honest or easier to reach, while Sakshi still only watches, nothing leaves the
machine and it works with zero configuration? Nothing here is promised; each is
small enough to be one pull request.

## Would fit

- **Fewer false positives and misses in the existing rules.** The most valuable
  work there is. Reports come through the "Trust rule" issue template.
- **More ways to fake success, as they are seen in the wild.** For example a
  test command swapped for one that runs fewer tests, or a CI step made
  non-blocking.
- **Codex live state on Windows.** Parked: the hook needs a POSIX shell and the
  plugin files are frozen.

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
