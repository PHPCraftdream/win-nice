# Code review — 2026-09-01

## Scope

Reviewed the Node.js installer/uninstaller and the Windows command wrappers in
`bin/`. The repository was also checked with both available test suites:

- Node.js: 23/23 tests passed.
- Pester: 16/16 tests passed.

The package treats installed scripts as managed package files. Reinstallation
may overwrite them; preserving local modifications is not part of the contract.

## Findings

### P1 — command wrappers do not preserve `cmd.exe` metacharacters

The wrappers forward an unprocessed `%*`, which is parsed again by `cmd.exe`.
Arguments containing `&`, `|`, `<`, `>`, `^`, or `%` can be changed or split
into separate commands.

For example, a quoted `A&B` argument passed through `cap` was split: `A` was
handled by the wrapped command and `B` was executed as another command. With
`idle` and `belownormal`, a separated command may also run outside the requested
priority class.

Relevant code:

- [`bin/cap.bat`](../../bin/cap.bat#L4)
- [`bin/idle.bat`](../../bin/idle.bat#L8)
- [`bin/belownormal.bat`](../../bin/belownormal.bat#L8)

### P1 — `cap` cannot forward some ordinary child-process flags

PowerShell binds arguments to `cap.ps1` before they reach `$Command`. Some flags
intended for the child process are consequently interpreted as abbreviated
PowerShell common parameters.

For example, `cap 50 node -e 0` fails because `-e` is treated as an ambiguous
abbreviation of `-ErrorAction` and `-ErrorVariable`. Other common parameters,
such as `-Verbose` or `-Debug`, may be consumed instead of forwarded.

Relevant code:

- [`bin/cap.ps1`](../../bin/cap.ps1#L3)
- [`bin/cap.bat`](../../bin/cap.bat#L4)

### P2 — uninstall trusts paths stored in the manifest

`data.binDir` and entries in `data.files` are joined and passed to the deletion
logic without verifying that the resolved paths remain inside the current
installation directory. A corrupted or modified manifest can therefore direct
uninstall outside the install root.

Before deletion, paths should be canonicalized and checked against the expected
`binDir`; absolute file names and traversal through `..` should be rejected.

Relevant code:

- [`install/uninstall.js`](../../install/uninstall.js#L21)

### P2 — uninstall behavior and documentation should follow the managed-file contract

The README currently promises that locally modified scripts survive uninstall
and reinstall, while the intended contract is that package files are managed
and may be replaced during updates.

The promise should be removed from the README. For consistent uninstall
behavior, files recorded in a valid manifest can be treated as owned by the
package and removed regardless of their current contents. The marker check is
still useful for the fallback directory scan when the manifest is missing or
invalid, because that scan may encounter unrelated files.

Relevant code and documentation:

- [`README.md`](../../README.md#L80)
- [`install/uninstall.js`](../../install/uninstall.js#L7)

## Test coverage gaps

Add integration cases covering:

- arguments containing `&`, `|`, quotes, carets, percent signs, and empty values;
- child commands whose arguments overlap PowerShell common parameters;
- manifest entries containing absolute paths or `..` traversal;
- the chosen managed-file semantics for install, reinstall, and uninstall.
