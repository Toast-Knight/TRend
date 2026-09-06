@echo off
setlocal

set "MEDIA=%~1"
if not defined MEDIA if exist "%~dp0..\..\samples\ColorTest.gif" set "MEDIA=%~dp0..\..\samples\ColorTest.gif"
if not defined MEDIA set "MEDIA=%~dp0..\..\tests\fixtures\media\ColorTest.gif"

powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass ^
  -File "%~dp0play-top-right.ps1" ^
  -Media "%MEDIA%" ^
  -Width 15 ^
  -Height 15

exit /b %ERRORLEVEL%