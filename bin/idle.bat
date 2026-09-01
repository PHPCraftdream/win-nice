@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: idle ^<command^> [args...] 1>&2
    exit /b 1
)
start "" /low /b /wait %*
exit /b %ERRORLEVEL%
