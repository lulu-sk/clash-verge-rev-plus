@echo off
setlocal

cd /d "%~dp0"

where pwsh >nul 2>nul
if %ERRORLEVEL%==0 (
  pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\sync-build-install.ps1"
) else (
  echo PowerShell 7 was not found. Please install PowerShell 7, then run this file again.
  set EXIT_CODE=1
  goto END
)

set EXIT_CODE=%ERRORLEVEL%

:END
echo.
if not "%EXIT_CODE%"=="0" (
  echo Script failed. Exit code: %EXIT_CODE%
) else (
  echo Done.
)
pause
exit /b %EXIT_CODE%
