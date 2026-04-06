@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start-blink.ps1" %*
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo ERROR: Start failed. See logs above.
    pause
    exit /b %ERRORLEVEL%
)
echo.
echo Blink is running. Press any key to close this window.
pause
