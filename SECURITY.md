# Security Policy

## Reporting a vulnerability

Please report security issues privately via [GitHub Security Advisories](https://github.com/PHPCraftdream/win-nice/security/advisories/new)
rather than a public issue. If that's not available, open a regular issue asking
for a private contact channel - don't post exploit details publicly.

Expect an initial response within a few days. This is a solo-maintained project,
so timelines are best-effort, not contractual.

## Scope

In scope: bugs in the `wnice.exe` binary (`src/`), the generated Git Bash shims or `install/*.js`
that let an attacker escalate privileges, execute unintended commands, or bypass
the argument-safety guarantees described in README's "Argument handling" section
beyond what's already documented as a known limitation there.

Out of scope (already known, not vulnerabilities):
- `cy`/`cx` intentionally run `claude`/`codex` with permission/approval bypass
  flags - that's the documented purpose of those two tools, not a bug.
- `realtime` intentionally requests `REALTIME_PRIORITY_CLASS`, which can freeze
  the desktop if misused - documented and warned about in README.
- `admin` intentionally elevates via UAC - that's its documented purpose.
- `%VAR%` expansion done by cmd.exe on its own command line before a tool
  starts, and Windows PowerShell 5.1's mis-quoting of a trailing backslash -
  documented calling-shell behavior, not a win-nice bug.

## Threat-model notes for the 0.4 features

### `wnice --wsl` (WSL2 mode, new in 0.4.0)

`wnice --wsl` invokes `wsl.exe -u root` to create a transient systemd scope
in the guest distro, then drops to the distro's default user (`runuser -u`)
before the wrapped command starts. Root is involved only in creating the
scope: wnice passes exactly the systemd properties mapped from its own
flags (`CPUQuota`, `CPUWeight`/`Nice`, `MemoryMax`, `AllowedCPUs`,
`TasksMax`, `RuntimeMaxSec`, `KillSignal`) plus a fixed unit name derived
from wnice's own PID (`wnice-<pid>-<ms>`).

- The wrapped command runs as the distro's **default user, not root**.
- There is **no shell on either side**: the chain travels as an argv array
  (`wsl.exe --exec`, then systemd-run/runuser `--` passthrough), so there
  is no quoting or interpolation boundary to inject through. User input
  appears only as the arguments after `--` and as the `--wsl=<distro>`
  name, which is matched against `wsl.exe`'s own distro list.
- What it does **not** do: cgroup limits are resource limits, **not a
  security sandbox**. A command run under `--wsl` can do anything the
  default user can do in that guest - read/write its files, use its
  network, signal its processes. If you would not run a command directly
  in the distro, do not run it under `--wsl` either.
- Root inside the guest (or anyone able to run `systemd-run`) can change
  or remove a scope's properties; the scope is not tamper-proof against
  the guest's own administrator.

### Priority booster (`--boost`, and the un-elevated above-normal path)

The booster needs no privileges and performs no elevation. It opens only
processes of its own job's tree (same user) with
`PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION` and calls
`SetPriorityClass` on them. It cannot raise the priority of another user's
processes or of system processes, and it never applies
`REALTIME_PRIORITY_CLASS` (`realtime` still downgrades to `HIGH`). No
elevation prompt is ever shown and no privilege is requested or held.

## Supported versions

Only the latest published version on npm is supported - 0.4.x as of this
writing; earlier versions (including 0.3.x) are not. This project is
pre-1.0; no long-term-support branches exist yet.
