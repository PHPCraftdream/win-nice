@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
:: DANGEROUS: see realtime.ps1 and README before using this.
if "%~1"=="" (
    echo usage: realtime ^<command^> [args...] 1>&2
    exit /b 1
)
:: A literal "%" in any argument gets corrupted here - see idle.bat for why (a
:: cmd.exe batch-parameter quirk, not fixable from inside a .bat). Every other
:: cmd.exe metacharacter (&|<>^) survives this hop untouched. Invoking "realtime"
:: bare from an actual PowerShell session skips this file entirely (PowerShell
:: prefers realtime.ps1, which doesn't have this problem).
start "" /realtime /b /wait %*
exit /b %ERRORLEVEL%
