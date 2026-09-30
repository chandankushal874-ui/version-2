@echo off
title Ollalink Translate - Security & Certificate Trust Installer
echo ========================================================
echo   Ollalink Translate - Trust Certificate Installer
echo ========================================================
echo.
echo Installing security certificate into Windows Trusted Root store...
echo This permanently whitelists this app on this PC and prevents
echo Windows Smart App Control and SmartScreen from blocking it.
echo.
certutil -addstore -f "Root" "%~dp0ollalink-cert.cer" >nul 2>&1
certutil -addstore -f "TrustedPublisher" "%~dp0ollalink-cert.cer" >nul 2>&1
powershell -NoProfile -Command "Get-ChildItem -Path '%~dp0' -Recurse | Unblock-File" >nul 2>&1
echo [SUCCESS] Certificate installed and files unblocked!
echo You can now run ollalink-translate.exe or Ollalink-Translate-Setup.exe without any blocks.
echo.
pause