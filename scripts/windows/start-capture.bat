@echo off
rem Starts only the capture agent, for a PC whose rDownloader server runs elsewhere -- a NAS, a
rem Docker host. Pair it once: in that server's web interface, Settings ^> Desktop client shows the
rem command. Same as "start-rdownloader.bat capture"; this file exists to be double-clicked.
setlocal
rem The called script keeps its window open on a failure only when it was double-clicked itself;
rem here this file was, so it holds the window instead.
set "RDOWNLOADER_NO_PAUSE=1"
call "%~dp0start-rdownloader.bat" capture
set "RESULT=%ERRORLEVEL%"
if not "%RESULT%"=="0" echo %cmdcmdline% | find /i "%~nx0" >nul && pause
exit /b %RESULT%
