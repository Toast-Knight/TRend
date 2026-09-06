@echo off
setlocal EnableExtensions EnableDelayedExpansion

set "HARNESS_DIR=%~dp0"
set "MATERIALS=%~dp0..\tests\fixtures\media"
set "REPO_ROOT=%~dp0.."
set "VIEWER=%REPO_ROOT%\target\debug\trend-test-viewer.exe"
set "VIEWER_READY="
set "CARGO=cargo"
where cargo >nul 2>&1
if errorlevel 1 if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"

if not exist "%MATERIALS%\" (
    echo Materials directory not found:
    echo "%MATERIALS%"
    pause
    exit /b 1
)

call :ensure_viewer
if errorlevel 1 (
    pause
    exit /b 1
)

:menu
cls
echo ========================================
echo              TRend Media Tests
echo ========================================
echo.
set /a COUNT=0
for %%F in ("%MATERIALS%\*") do (
    set /a COUNT+=1
    set "FILE[!COUNT!]=%%~fF"
    set "SUPPORT=[NOT IMPLEMENTED]"
    "%VIEWER%" --supports "%%~xF" >nul 2>&1 && set "SUPPORT=[SUPPORTED]"
    echo !COUNT!. %%~nxF !SUPPORT!
)
echo.
echo A. Test ALL
echo Q. Quit
echo.
set "CHOICE="
set /p "CHOICE=Select: "

if /i "%CHOICE%"=="Q" exit /b 0
if /i "%CHOICE%"=="A" (
    call :select_quality
    if errorlevel 1 goto menu
    for /l %%I in (1,1,!COUNT!) do call :test_file "!FILE[%%I]!" all
    echo.
    echo All media tests completed.
    pause
    goto menu
)

for /f "delims=0123456789" %%A in ("%CHOICE%") do goto invalid
if "%CHOICE%"=="" goto invalid
if %CHOICE% LSS 1 goto invalid
if %CHOICE% GTR %COUNT% goto invalid
call :select_quality
if errorlevel 1 goto menu
call :test_file "!FILE[%CHOICE%]!" single
goto menu

:invalid
echo Invalid selection.
pause
goto menu

:test_file
set "MEDIA_FILE=%~1"
set "RUN_MODE=%~2"
set "EXT=%~x1"
set "EXT=!EXT:~1!"

"%VIEWER%" --supports "!EXT!" >nul 2>&1 && goto supported
if /i "!EXT!"=="webp" set "FORMAT=WEBP" & set "STATUS=Image adapter does not implement WebP yet." & goto unsupported
if /i "!EXT!"=="webm" set "FORMAT=WEBM" & set "STATUS=Video adapter not implemented yet." & goto unsupported
if /i "!EXT!"=="mkv" set "FORMAT=MKV" & set "STATUS=Video adapter not implemented yet." & goto unsupported
set "FORMAT=!EXT!"
set "STATUS=No TRend adapter is mapped for this format."

:unsupported
cls
echo TRend Media Test
echo ----------------------------------------
echo.
echo File: %~nx1
echo Format: !FORMAT!
echo Status: !STATUS!
echo.
if /i "!RUN_MODE!"=="all" (
    timeout /t 2 /nobreak ^>nul
) else (
    pause
)
exit /b 0

:supported
if /i "!QUALITY_MODE!"=="compare" (
    "%VIEWER%" --compare-all "%MEDIA_FILE%"
) else (
    "%VIEWER%" --quality "!QUALITY_MODE!" "%MEDIA_FILE%"
)
if errorlevel 1 (
    echo.
    echo The viewer exited with an error.
    if /i not "!RUN_MODE!"=="all" pause
)
exit /b 0

:ensure_viewer
if defined VIEWER_READY exit /b 0
echo Building the TRend developer test viewer...
pushd "%REPO_ROOT%"
"%CARGO%" build --quiet -p trend-test-viewer
set "BUILD_RESULT=!ERRORLEVEL!"
popd
if not "!BUILD_RESULT!"=="0" (
    echo.
    echo Could not build trend-test-viewer.
    echo Install the Rust toolchain or run cargo build -p trend-test-viewer manually.
    exit /b 1
)
set "VIEWER_READY=1"
exit /b 0

:select_quality
cls
echo ========================================
echo          TRend Quality Selection
echo ========================================
echo.
echo   1. FAST             ^(nearest half-block^)
echo   2. BALANCED         ^(bilinear half-block^)
echo   3. QUALITY          ^(area half-block^)
echo.
echo Comparison
echo   4. COMPARE ALL      ^(starts FAST; animation/video starts paused; M cycles^)
echo   B. Back
echo.
set "QUALITY_CHOICE="
set /p "QUALITY_CHOICE=Select quality: "
if /i "!QUALITY_CHOICE!"=="B" exit /b 1
if "!QUALITY_CHOICE!"=="1" set "QUALITY_MODE=fast" & exit /b 0
if "!QUALITY_CHOICE!"=="2" set "QUALITY_MODE=balanced" & exit /b 0
if "!QUALITY_CHOICE!"=="3" set "QUALITY_MODE=quality" & exit /b 0
if "!QUALITY_CHOICE!"=="4" set "QUALITY_MODE=compare" & exit /b 0
echo.
echo Invalid quality selection.
pause
goto select_quality