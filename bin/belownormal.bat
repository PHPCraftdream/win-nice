@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: belownormal ^<command^> [args...] 1>&2
    exit /b 1
)
:: A literal "%" in any argument gets corrupted here - see capc.bat for why (a
:: cmd.exe batch-parameter quirk, not fixable from inside a .bat). Every other
:: cmd.exe metacharacter (&|<>^) survives this hop untouched. Invoking
:: "belownormal" bare from an actual PowerShell session skips this file
:: (belownormal.ps1 preferred).
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0belownormal.ps1" %*
