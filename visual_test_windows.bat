@echo off
setlocal EnableExtensions EnableDelayedExpansion

cd /d "%~dp0"
set "REPO_ROOT=%CD%"
set "MATERIALS=%REPO_ROOT%\Test runs Envs\Materials"
set "VIEWER=%REPO_ROOT%\target\debug\trend-test-viewer.exe"
set "FEATURES=webp,webm,mkv,avi,mov"

if not exist "%MATERIALS%\" (
    echo ERROR: Materials directory not found: "%MATERIALS%"
    exit /b 1
)
where cargo >nul 2>&1
if errorlevel 1 (
    echo ERROR: Cargo was not found on PATH.
    exit /b 1
)

echo Building the Windows visual test viewer...
cargo build --quiet -p trend-test-viewer --features %FEATURES%
if errorlevel 1 (
    echo ERROR: Could not build the Windows viewer.
    exit /b 1
)

:menu
cls
echo ========================================
echo TRend Windows Interactive Visual Tests
echo ========================================
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
        set "DISPLAY=%%~fF"
        set "DISPLAY=!DISPLAY:%MATERIALS%\=!"
        echo !COUNT!. !DISPLAY!
    )
)
if !COUNT! EQU 0 (
    echo No supported media files were found.
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
if /i "!CHOICE!"=="R" goto rebuild
if /i "!CHOICE!"=="A" (
    call :select_quality
    if errorlevel 1 goto menu
    for /l %%I in (1,1,!COUNT!) do (
        call :open_media "!FILE[%%I]!"
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
call :open_media "!FILE[%CHOICE%]!"
goto menu

:is_supported
set "EXTENSION=%~1"
set "EXTENSION=%EXTENSION:.=%"
call set "CACHED=%%SUPPORT_%EXTENSION%%%"
if defined CACHED (
    if "%CACHED%"=="1" exit /b 0
    exit /b 1
)
"%VIEWER%" --supports "%~1" >nul 2>&1
if errorlevel 1 (
    set "SUPPORT_%EXTENSION%=0"
    exit /b 1
)
set "SUPPORT_%EXTENSION%=1"
exit /b 0

:rebuild
echo Rebuilding the Windows visual test viewer...
cargo build --quiet -p trend-test-viewer --features %FEATURES%
if errorlevel 1 pause
goto menu

:invalid
echo Invalid selection.
pause
goto menu

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
cls
echo Opening: %~1
echo Viewer controls: Q/Esc back, Space pause, arrows seek, 1-3 quality, M cycle.
if /i "!QUALITY_MODE!"=="compare" (
    "%VIEWER%" --compare-all "%~1"
) else (
    "%VIEWER%" --quality "!QUALITY_MODE!" "%~1"
)
if errorlevel 1 (
    echo.
    echo Viewer returned an error for: %~1
    pause
    exit /b 1
)
exit /b 0
