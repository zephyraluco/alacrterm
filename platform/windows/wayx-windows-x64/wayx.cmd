@echo off
setlocal
set "WAYX_XKB_DIR=%~dp0share\X11\xkb"
set "WAYX_XKB_BIN_DIR=%~dp0bin"
set "PATH=%~dp0bin;%PATH%"
"%~dp0bin\wayx.exe" serve --core "%~dp0bin\libwayx-xserver.dll" %*
exit /b %ERRORLEVEL%
