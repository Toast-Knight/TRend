@echo off
setlocal EnableExtensions

cd /d "%~dp0.."
if not defined TREND_WSL_DISTRO set "TREND_WSL_DISTRO=Debian"

where wsl.exe >nul 2>&1
if errorlevel 1 (
    echo ERROR: WSL2 was not found.
    exit /b 1
)

echo.
echo ========================================
echo TRend Linux build
echo Distribution: %TREND_WSL_DISTRO%
echo ========================================
echo.

call :wsl "bash scripts/build-linux.sh"
if errorlevel 1 exit /b 1

echo.
echo Linux release build complete.
echo Output: dist\linux
exit /b 0

:wsl
echo.
echo ^> WSL2: %~1
wsl.exe -d %TREND_WSL_DISTRO% --cd "%CD%" -- bash -lc "%~1"
set "WSL_RESULT=%ERRORLEVEL%"
if not "%WSL_RESULT%"=="0" (
    echo ERROR: WSL2 command failed.
    exit /b %WSL_RESULT%
)
exit /b 0
