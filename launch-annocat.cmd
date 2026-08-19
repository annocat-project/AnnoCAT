@echo off
setlocal
if not exist "%~dp0annocat.exe" (
  echo AnnoCAT could not find annocat.exe.
  echo Extract the complete release ZIP before running this launcher.
  pause
  exit /b 1
)
"%~dp0annocat.exe" launch
if errorlevel 1 pause
