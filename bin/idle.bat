@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: idle ^<command^> [args...] 1>&2
    exit /b 1
)
:: A literal "%" in any argument gets corrupted here - confirmed with nothing more
:: than a bare "echo %1", a cmd.exe batch-parameter quirk not fixable from inside a
:: .bat. Every other cmd.exe metacharacter (&|<>^) survives this hop untouched.
:: Invoking "idle" bare from an actual PowerShell session skips this file entirely
:: (PowerShell prefers idle.ps1, which doesn't have this problem).
start "" /low /b /wait %*
exit /b %ERRORLEVEL%
