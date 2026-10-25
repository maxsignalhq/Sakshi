# Security

Sakshi sits between you and AI agents that can run commands, so its own safety
matters. Thank you for reporting problems responsibly.

## Reporting a vulnerability

Please **do not open a public issue.** Use GitHub's private reporting instead:
go to the repository's **Security** tab and choose **Report a vulnerability**
(or open https://github.com/maxsignalhq/Sakshi/security/advisories/new).

Include what you found, how to reproduce it, and what an attacker could do with
it. You can expect an acknowledgement within a few days. Once a fix is released
you will be credited in the advisory, unless you prefer not to be.

## Supported versions

Sakshi is pre-1.0. Fixes go into the latest release only.

## What is in scope

- **The local IPC endpoint.** The Unix socket (private directory, mode 0600) and
  the Windows named pipe (remote clients rejected) should only accept the current
  user. Anything that lets another user or a remote party inject events, read
  them, or crash Sakshi is in scope.
- **Hooks.** `sakshi hook` must fail open and must never block, slow or alter what
  an agent does. A way to make it do any of those is in scope.
- **Generated agent config.** Sakshi writes a settings file for Claude Code into
  its private runtime directory. Problems with how that file is written or read
  are in scope.
- **Activity logs and config parsing.** Crafted logs or config files that cause
  code execution or memory-safety problems.
- **Install scripts and release artifacts.** Anything that defeats checksum
  verification in `install.sh` / `install.ps1`.

## What is not

- **Trust findings being wrong.** The trust rules are heuristics; a missed or
  false finding is a bug, not a vulnerability. Please use the "Trust rule" issue
  template.
- What an agent itself does. Sakshi observes agents; it does not sandbox them.
