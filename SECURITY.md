# Security Policy

## Reporting a vulnerability

Please report security issues privately via [GitHub Security Advisories](https://github.com/PHPCraftdream/win-nice/security/advisories/new)
rather than a public issue. If that's not available, open a regular issue asking
for a private contact channel - don't post exploit details publicly.

Expect an initial response within a few days. This is a solo-maintained project,
so timelines are best-effort, not contractual.

## Scope

In scope: bugs in `bin/*.ps1`/`bin/*.bat`/the extensionless shims or `install/*.js`
that let an attacker escalate privileges, execute unintended commands, or bypass
the argument-safety guarantees described in README's "Argument handling" section
beyond what's already documented as a known limitation there.

Out of scope (already known, not vulnerabilities):
- `cy`/`cx` intentionally run `claude`/`codex` with permission/approval bypass
  flags - that's the documented purpose of those two tools, not a bug.
- `realtime` intentionally requests `REALTIME_PRIORITY_CLASS`, which can freeze
  the desktop if misused - documented and warned about in README.
- `admin` intentionally elevates via UAC - that's its documented purpose.
- The `.bat` entry points' literal-`%`-corruption quirk - a documented,
  unfixable-from-inside-a-`.bat` cmd.exe behavior, not a win-nice bug.

## Supported versions

Only the latest published version on npm is supported. This project is pre-1.0;
no long-term-support branches exist yet.
