@echo off
title Ollalink Translate Release Launcher
echo ========================================================
echo   Ollalink Translate - Cloud Relay Mode (Render)
echo ========================================================
echo.
echo [1/2] Unblocking files from Smart App Control / SmartScreen...
powershell -NoProfile -Command "Get-ChildItem -Path '%~dp0' -Recurse | Unblock-File" 2>nul

echo.
echo [2/2] Launching Ollalink Translate Desktop App (Connected to Cloud Relay)...
if exist "%~dp0ollalink-translate.exe" (
    start "" "%~dp0ollalink-translate.exe"
) else (
    start "" "%~dp0app\src-tauri\target\release\ollalink-translate.exe"
)
