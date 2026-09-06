@echo off
setlocal EnableExtensions

call "%~dp0build-windows.bat"
if errorlevel 1 exit /b 1

call "%~dp0build-linux-wsl2.bat"
if errorlevel 1 exit /b 1

echo.
echo ========================================
echo All target builds packaged
echo ========================================
echo.
echo Windows: dist\windows
echo Linux: dist\linux
echo.
exit /b 0
