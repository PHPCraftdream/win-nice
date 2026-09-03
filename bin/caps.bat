@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
:: A literal "%" in any argument gets corrupted here (cmd.exe rescans %1/%* for
:: %...% patterns the moment a batch file reads them - confirmed with nothing more
:: than a bare "echo %1", no forwarding involved; there's no per-character escape
:: for this from inside a .bat). Every other cmd.exe metacharacter (&|<>^) survives
:: this hop untouched. Invoking "caps" bare from an actual PowerShell session skips
:: this file entirely (PowerShell prefers caps.ps1) and has no "%" problem at all.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0caps.ps1" %*
