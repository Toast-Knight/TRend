@echo off
setlocal EnableExtensions EnableDelayedExpansion

cd /d "%~dp0"
set "REPO_ROOT=%CD%"
set "MATERIALS=%REPO_ROOT%\Test runs Envs\Materials"
set "LINUX_VIEWER=../../target/debug/trend-test-viewer"
set "FEATURES=webp,linux-webm,linux-mkv,linux-h264"
if not defined TREND_WSL_DISTRO set "TREND_WSL_DISTRO=Debian"

if not exist "%MATERIALS%\" (
    echo ERROR: Materials directory not found: "%MATERIALS%"
    exit /b 1
)
where wsl.exe >nul 2>&1
if errorlevel 1 (
    echo ERROR: WSL2 was not found.
    exit /b 1
)

call :preflight
if errorlevel 1 exit /b 1
call :build
if errorlevel 1 exit /b 1

:menu
cls
echo ========================================
echo TRend WSL2 Linux Interactive Visual Tests
echo ========================================
echo Distribution: %TREND_WSL_DISTRO%
echo Materials: %MATERIALS%
echo.
set /a COUNT=0
for /r "%MATERIALS%" %%F in (*) do (
    set "NAME=%%~nF"
    set "VISUAL_FILE=1"
    if not "!NAME:corrupt=!"=="!NAME!" set "VISUAL_FILE=0"
    if "!VISUAL_FILE!"=="1" call :is_supported "%%~xF"
    if "!VISUAL_FILE!"=="1" if not errorlevel 1 (
        set /a COUNT+=1
        set "FILE[!COUNT!]=%%~fF"
        set "RELATIVE=%%~fF"
        set "RELATIVE=!RELATIVE:%MATERIALS%\=!"
        set "DISPLAY[!COUNT!]=!RELATIVE!"
        echo !COUNT!. !RELATIVE!
    )
)
if !COUNT! EQU 0 (
    echo No supported Linux media files were found.
    exit /b 1
)
echo.
echo A. Open every supported file in sequence
echo R. Rebuild viewer
echo Q. Quit
echo.
set "CHOICE="
set /p "CHOICE=Select media: "

if /i "!CHOICE!"=="Q" exit /b 0
if /i "!CHOICE!"=="R" (
    call :build
    if errorlevel 1 pause
    goto menu
)
if /i "!CHOICE!"=="A" (
    call :select_quality
    if errorlevel 1 goto menu
    for /l %%I in (1,1,!COUNT!) do (
        call :open_media "!DISPLAY[%%I]!"
        if errorlevel 1 goto menu
    )
    goto menu
)
for /f "delims=0123456789" %%A in ("!CHOICE!") do goto invalid
if "!CHOICE!"=="" goto invalid
if !CHOICE! LSS 1 goto invalid
if !CHOICE! GTR !COUNT! goto invalid
call :select_quality
if errorlevel 1 goto menu
call :open_media "!DISPLAY[%CHOICE%]!"
goto menu

:is_supported
set "EXTENSION=%~1"
set "EXTENSION=%EXTENSION:.=%"
call set "CACHED=%%SUPPORT_%EXTENSION%%%"
if defined CACHED (
    if "%CACHED%"=="1" exit /b 0
    exit /b 1
)
wsl.exe -d %TREND_WSL_DISTRO% --cd "%MATERIALS%" -- %LINUX_VIEWER% --supports "%~1" <nul >nul 2>&1
if errorlevel 1 (
    set "SUPPORT_%EXTENSION%=0"
    exit /b 1
)
set "SUPPORT_%EXTENSION%=1"
exit /b 0

:invalid
echo Invalid selection.
pause
goto menu

:preflight
echo Checking WSL2 Linux dependencies in %TREND_WSL_DISTRO%...
wsl.exe -d %TREND_WSL_DISTRO% -- bash -lc "cargo --version"
if errorlevel 1 (
    echo ERROR: Cargo is unavailable in the WSL distribution.
    exit /b 1
)
wsl.exe -d %TREND_WSL_DISTRO% -- bash -lc "pkg-config --exists vpx"
if errorlevel 1 (
    echo ERROR: libvpx development files are missing.
    echo Install them with: sudo apt install pkg-config libvpx-dev
    exit /b 1
)
wsl.exe -d %TREND_WSL_DISTRO% -- bash -lc "compgen -G '/lib/*/libopenh264.so*' || compgen -G '/usr/lib/*/libopenh264.so*'"
if errorlevel 1 (
    echo ERROR: The OpenH264 runtime is missing.
    echo On Debian install it with: sudo apt install libopenh264-8
    exit /b 1
)
exit /b 0

:build
echo Building the WSL2 Linux visual test viewer...
wsl.exe -d %TREND_WSL_DISTRO% --cd "%REPO_ROOT%" -- bash -lc "cargo build --quiet -p trend-test-viewer --features %FEATURES%"
if errorlevel 1 (
    echo ERROR: Could not build the WSL2 Linux viewer.
    exit /b 1
)
exit /b 0

:select_quality
cls
echo ========================================
echo Select Visual Quality
echo ========================================
echo 1. FAST       - nearest sampling
echo 2. BALANCED   - bilinear sampling
echo 3. QUALITY    - area sampling
echo 4. COMPARE    - starts paused; press M to cycle
echo B. Back
echo.
set "QUALITY_CHOICE="
set /p "QUALITY_CHOICE=Select quality: "
if /i "!QUALITY_CHOICE!"=="B" exit /b 1
if "!QUALITY_CHOICE!"=="1" set "QUALITY_MODE=fast" & exit /b 0
if "!QUALITY_CHOICE!"=="2" set "QUALITY_MODE=balanced" & exit /b 0
if "!QUALITY_CHOICE!"=="3" set "QUALITY_MODE=quality" & exit /b 0
if "!QUALITY_CHOICE!"=="4" set "QUALITY_MODE=compare" & exit /b 0
echo Invalid quality selection.
pause
goto select_quality

:open_media
set "LINUX_MEDIA=%~1"
set "LINUX_MEDIA=!LINUX_MEDIA:\=/!"
cls
echo Opening in WSL2: %~1
echo Viewer controls: Q/Esc back, Space pause, arrows seek, 1-3 quality, M cycle.
if /i "!QUALITY_MODE!"=="compare" (
    wsl.exe -d %TREND_WSL_DISTRO% --cd "%MATERIALS%" -- %LINUX_VIEWER% --compare-all "!LINUX_MEDIA!"
) else (
    wsl.exe -d %TREND_WSL_DISTRO% --cd "%MATERIALS%" -- %LINUX_VIEWER% --quality "!QUALITY_MODE!" "!LINUX_MEDIA!"
)
if errorlevel 1 (
    echo.
    echo Viewer returned an error for: %~1
    pause
    exit /b 1
)
exit /b 0
