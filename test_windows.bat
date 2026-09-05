@echo off
setlocal EnableExtensions

cd /d "%~dp0"

echo ========================================
echo TRend Windows capability tests
echo ========================================
echo.

where cargo >nul 2>&1
if errorlevel 1 (
    echo ERROR: Cargo was not found on PATH.
    exit /b 1
)

call :run cargo test --workspace --all-features
if errorlevel 1 exit /b 1

call :run cargo clippy --workspace --all-targets --all-features -- -D warnings
if errorlevel 1 exit /b 1

call :run cargo fmt --all -- --check
if errorlevel 1 exit /b 1

call :probe "Test runs Envs\Materials\ColorTest.mp4"
if errorlevel 1 exit /b 1
call :probe "Test runs Envs\Materials\WebM Experimental\vp8-normal.webm"
if errorlevel 1 exit /b 1
call :probe "Test runs Envs\Materials\MKV Experimental\h264-normal.mkv"
if errorlevel 1 exit /b 1
call :probe "Test runs Envs\Materials\AVI Experimental\mjpeg-normal.avi"
if errorlevel 1 exit /b 1
call :probe "Test runs Envs\Materials\MOV Experimental\hevc-large.mov"
if errorlevel 1 exit /b 1

echo.
echo ========================================
echo Windows capability tests PASSED
echo MP4, WebM, MKV, AVI, and MOV rendered.
echo ========================================
exit /b 0

:run
echo.
echo ^> %*
%*
if errorlevel 1 (
    echo ERROR: Command failed.
    exit /b 1
)
exit /b 0

:probe
set "MEDIA=%~1"
if not exist "%MEDIA%" (
    echo ERROR: Missing fixture: %MEDIA%
    exit /b 1
)
echo.
echo ^> Probing %MEDIA%
cargo run --quiet -p trend-test-viewer --features webp,webm,mkv,avi,mov -- --probe 20x10 2 "%MEDIA%"
if errorlevel 1 (
    echo ERROR: Viewer probe failed: %MEDIA%
    exit /b 1
)
exit /b 0
