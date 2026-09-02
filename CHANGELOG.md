# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `idle`, `belownormal`, `abovenormal`, `high`, `realtime` - Windows priority
  class launchers.
- `cap` - hard CPU quota via Job Objects.
- `pint` - CPU affinity ("pin threads") via Job Objects.
- `admin` - run a command elevated, with UAC prompt when needed.
- `uiup` - one-shot desktop-responsiveness priority boost.
- `cy` / `cx` - `claude`/`codex` launchers with permission/approval bypass
  flags, for use in already-sandboxed/disposable environments.
- Each tool ships a `.bat`, `.ps1`, and extensionless (Git Bash) entry point.
- npm-based installer (`postinstall`/`preuninstall`, `win-nice status
  |reinstall|uninstall`), PATH management via the Windows registry.
- Optional `win-nice skill install|uninstall` - installs a reference skill
  for Claude Code and Codex CLI.
- Node test suite (installer logic) and Pester integration suite (real
  Windows process/priority/Job-Object behavior).
