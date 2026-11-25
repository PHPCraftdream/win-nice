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

## Threat-model notes for the 0.5 features

All of these are opt-in and run with the caller's own rights: nothing here
elevates, and nothing reaches a process the same user could not already
open.

### `wnice --jobs` and `wnice --audit` (read-only)

- `--jobs` modifies nothing. It finds Job Objects through the system handle
  table and duplicates handles with query-only rights, and only from
  wnice-family processes of the same user: the owner's user SID is checked
  before anything else is read from a process, and foreign owners are only
  counted.
- The report prints each wrapper's command line (cut at 1024 characters).
  Arguments may hold secrets, so treat the output like the command line
  itself before pasting it into an issue.
- `--audit` only reads job accounting and the same user's process list; it
  touches no process.

### `wnice --adopt`

- It opens only processes of the same user, with
  `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA | PROCESS_TERMINATE`,
  and assigns them to the command's job. Other users' processes are skipped
  without output; wnice's own process, its ancestors and the root command
  are never adopted.
- Selection is by exact image name, so every same-user process with that
  name is taken, not only the one meant. An adopted process keeps the job's
  limits until it exits and is killed with the job on `--timeout` or when
  wnice itself is killed. The job's memory limit counts it too, so a large
  server may start failing allocations.

### `WNICE_AGENT_LIMITS`

- The value goes through the same parser as the `wnice` command line and
  accepts limit options and `--no-servers` only. A command, `--`, `--wsl`,
  `--jobs`, `--audit`, `--adopt` and the help/version flags are refused
  (exit 1, nothing started), so setting the variable cannot change which
  command `cy` or `cx` runs.

## Supported versions

Only the latest published version on npm is supported - 0.5.x as of this
writing; earlier versions (including 0.4.x) are not. This project is
pre-1.0; no long-term-support branches exist yet.

## Verifying artifacts

If a `wnice.exe` or an npm package reached you through some other channel,
[docs/VERIFYING.md](docs/VERIFYING.md) shows how to confirm it really came
from this repository (artifact attestations, checksums, npm provenance).
