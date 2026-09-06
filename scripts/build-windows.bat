@echo off
setlocal EnableExtensions

cd /d "%~dp0.."

where rustup >nul 2>&1
if errorlevel 1 (
    echo ERROR: rustup was not found on PATH.
    exit /b 1
)

echo.
echo ========================================
echo TRend Windows build
echo ========================================
echo.

echo ^> cargo build --workspace --all-features --release
cargo build --workspace --all-features --release
if errorlevel 1 (
    echo ERROR: Windows release build failed.
    exit /b 1
)

powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass ^
    -File "%~dp0package-windows.ps1"
if errorlevel 1 exit /b 1

echo.
echo Windows release build complete.
echo Output: dist\windows
exit /b 0
