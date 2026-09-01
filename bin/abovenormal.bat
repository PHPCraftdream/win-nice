@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: abovenormal ^<command^> [args...] 1>&2
    exit /b 1
)
:: A literal "%" in any argument gets corrupted here - see idle.bat for why (a
:: cmd.exe batch-parameter quirk, not fixable from inside a .bat). Every other
:: cmd.exe metacharacter (&|<>^) survives this hop untouched. Invoking
:: "abovenormal" bare from an actual PowerShell session skips this file entirely
:: (PowerShell prefers abovenormal.ps1, which doesn't have this problem).
start "" /abovenormal /b /wait %*
exit /b %ERRORLEVEL%
