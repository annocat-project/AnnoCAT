@echo off
setlocal
cd /d "%~dp0"
set "ANNOCAT_HOME=%~dp0"
if exist "annocat.exe" (
  "annocat.exe" launch
) else if exist "target\release\annocat.exe" (
  "target\release\annocat.exe" launch
) else if exist "target\debug\annocat.exe" (
  "target\debug\annocat.exe" launch
) else (
  where cargo >nul 2>nul
  if errorlevel 1 (
    echo AnnoCAT could not find annocat.exe.
    echo Extract the complete release ZIP before running this launcher.
    pause
    exit /b 1
  )
  cargo run -p annocat-cli -- launch
)
if errorlevel 1 pause
