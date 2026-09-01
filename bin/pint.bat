@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
:: A literal "%" in any argument gets corrupted here - see cap.bat for why (a
:: cmd.exe batch-parameter quirk, not fixable from inside a .bat). Every other
:: cmd.exe metacharacter (&|<>^) survives this hop untouched. Invoking "pint"
:: bare from an actual PowerShell session skips this file (pint.ps1 preferred).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0pint.ps1" %*
