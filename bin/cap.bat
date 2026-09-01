@echo off
:: SPDX-License-Identifier: MIT OR Apache-2.0
:: win-nice: managed-file
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0cap.ps1" %*
