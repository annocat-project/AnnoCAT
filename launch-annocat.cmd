@echo off
setlocal
cd /d "%~dp0"
set "ANNOCAT_HOME=%~dp0"
if exist "target\debug\annocat.exe" (
  "target\debug\annocat.exe" launch
) else (
  cargo run -p annocat-cli -- launch
)
if errorlevel 1 pause
