@echo off
setlocal EnableExtensions

cd /d "%~dp0.."

if not defined ANDROID_API_LEVEL set "ANDROID_API_LEVEL=21"

if defined ANDROID_NDK_HOME (
    set "NDK_ROOT=%ANDROID_NDK_HOME%"
) else if defined ANDROID_NDK_ROOT (
    set "NDK_ROOT=%ANDROID_NDK_ROOT%"
) else (
    for /d %%D in ("%CD%\android-ndk-*") do if not defined NDK_ROOT set "NDK_ROOT=%%~fD"
    if not defined NDK_ROOT (
        echo ERROR: Set ANDROID_NDK_HOME or place android-ndk-* in this directory.
        exit /b 1
    )
)

set "ANDROID_LINKER=%NDK_ROOT%\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android%ANDROID_API_LEVEL%-clang.cmd"
if not exist "%ANDROID_LINKER%" (
    echo ERROR: Android linker not found: %ANDROID_LINKER%
    exit /b 1
)

where rustup >nul 2>&1
if errorlevel 1 (
    echo ERROR: rustup was not found on PATH.
    exit /b 1
)

call rustup target add aarch64-linux-android
if errorlevel 1 exit /b 1

set "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=%ANDROID_LINKER%"
cargo build --workspace --all-features --release --target aarch64-linux-android
if errorlevel 1 exit /b 1

powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0package-android.ps1"
if errorlevel 1 exit /b 1

echo.
echo Android library: target\aarch64-linux-android\release\libtrend.so
echo Termux viewer: target\aarch64-linux-android\release\trend-test-viewer
echo Package: dist\android\aarch64-linux-android
exit /b 0