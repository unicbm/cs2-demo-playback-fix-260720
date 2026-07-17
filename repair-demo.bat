@echo off
setlocal
title CS2 Demo Playback Fix

if "%~1"=="" (
    echo Drag one or more CS2 .dem files onto this BAT file.
    echo.
    pause
    exit /b 2
)

set "FIX_EXE=%~dp0cs2-demo-playback-fix.exe"
if not exist "%FIX_EXE%" (
    echo ERROR: cs2-demo-playback-fix.exe was not found beside this BAT file.
    echo Expected: "%FIX_EXE%"
    echo.
    pause
    exit /b 2
)

"%FIX_EXE%" %*
set "FIX_EXIT=%ERRORLEVEL%"

echo.
if "%FIX_EXIT%"=="0" (
    echo Finished.
) else (
    echo Repair failed with exit code %FIX_EXIT%. The original demo was not changed.
)
pause
exit /b %FIX_EXIT%
