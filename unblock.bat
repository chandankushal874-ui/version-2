@echo off
echo ========================================================
echo   Unblocking Ollalink Translate Files
echo ========================================================
echo Removing Mark-of-the-Web (Zone.Identifier) to prevent Smart App Control blocks...
powershell -NoProfile -Command "Get-ChildItem -Path '%~dp0' -Recurse | Unblock-File"
echo.
echo [SUCCESS] Files unblocked! You can now run ollalink-translate.exe or Ollalink-Translate-Setup.exe.
pause