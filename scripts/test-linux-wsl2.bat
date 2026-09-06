@echo off
setlocal EnableExtensions

cd /d "%~dp0.."
if not defined TREND_WSL_DISTRO set "TREND_WSL_DISTRO=Debian"

echo ========================================
echo TRend WSL2 Linux capability tests
echo Distribution: %TREND_WSL_DISTRO%
echo ========================================
echo.

where wsl.exe >nul 2>&1
if errorlevel 1 (
    echo ERROR: WSL2 was not found.
    exit /b 1
)

call :wsl "cargo --version"
if errorlevel 1 (
    echo ERROR: Cargo is unavailable in the WSL distribution.
    exit /b 1
)

call :wsl "pkg-config --exists vpx"
if errorlevel 1 (
    echo ERROR: libvpx development files are missing.
    echo Install them with: sudo apt install pkg-config libvpx-dev
    exit /b 1
)

call :wsl "compgen -G '/lib/*/libopenh264.so*' || compgen -G '/usr/lib/*/libopenh264.so*'"
if errorlevel 1 (
    echo ERROR: The OpenH264 runtime is missing.
    echo On Debian install it with: sudo apt install libopenh264-8
    echo Other distributions may use a different OpenH264 package name.
    exit /b 1
)

call :wsl "cargo test --workspace --all-features"
if errorlevel 1 exit /b 1

call :wsl "cargo clippy --workspace --all-targets --all-features -- -D warnings"
if errorlevel 1 exit /b 1

call :wsl "cargo fmt --all -- --check"
if errorlevel 1 exit /b 1

call :wsl "cargo run --quiet -p trend-test-viewer --features linux-webm,linux-mkv,linux-h264 -- --probe 20x10 2 tests/fixtures/media/WebM\ Experimental/vp8-normal.webm"
if errorlevel 1 exit /b 1
call :wsl "cargo run --quiet -p trend-test-viewer --features linux-webm,linux-mkv,linux-h264 -- --probe 20x10 2 tests/fixtures/media/MKV\ Experimental/vp9-large.mkv"
if errorlevel 1 exit /b 1
call :wsl "cargo run --quiet -p trend-test-viewer --features linux-webm,linux-mkv,linux-h264 -- --probe 20x10 2 tests/fixtures/media/MKV\ Experimental/h264-linux-large.mkv"
if errorlevel 1 exit /b 1

echo.
echo ========================================
echo WSL2 Linux capability tests PASSED
echo WebM VP8, MKV VP9, and MKV H.264 rendered.
echo AVI and MOV rejection is covered by workspace tests.
echo ========================================
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
