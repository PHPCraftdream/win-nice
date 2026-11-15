# Support

This is a solo-maintained open source project: there is no paid support
channel, everything runs through GitHub.

## Getting help

- **Usage questions and bug reports:** open a GitHub issue
  (<https://github.com/PHPCraftdream/win-nice/issues>). Please include your
  Windows version, `wnice -V`, the shell you called the tool from, and the
  exact command that misbehaved (details in [CONTRIBUTING.md](CONTRIBUTING.md)).
- **Feature requests:** a GitHub issue describing the concrete use case.
- **Documentation:** [README.md](README.md) - and every tool has built-in
  help with examples (`wnice --help`, `capc --help`, `idle -h`, `admin /?`).
  Release history lives in [CHANGELOG.md](CHANGELOG.md); the optional Claude
  Code/Codex reference skill is documented in `skills/win-nice/SKILL.md`.

## Security

Anything resembling a security issue (privilege escalation, argument
injection beyond what's documented) must NOT be posted publicly - follow
[SECURITY.md](SECURITY.md) and report it privately via GitHub Security
Advisories.

## Supported versions

Only the latest version published on npm receives fixes; this is a pre-1.0
project with no long-term-support branches. Details in
[SECURITY.md](SECURITY.md).
