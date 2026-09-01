@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: abovenormal ^<command^> [args...] 1>&2
    exit /b 1
)
:: A literal "%" in any argument gets corrupted here - see cap.bat for why (a
:: cmd.exe batch-parameter quirk, not fixable from inside a .bat). Every other
:: cmd.exe metacharacter (&|<>^) survives this hop untouched. Invoking
:: "abovenormal" bare from an actual PowerShell session skips this file
:: (abovenormal.ps1 preferred).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0abovenormal.ps1" %*
