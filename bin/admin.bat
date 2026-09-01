@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
if "%~1"=="" (
    echo usage: admin ^<command^> [args...] 1>&2
    exit /b 1
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0admin.ps1" %*
