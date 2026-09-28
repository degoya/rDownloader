@echo off
rem Stops only the capture agent. Same as "stop-rdownloader.bat capture".
call "%~dp0stop-rdownloader.bat" capture
exit /b %ERRORLEVEL%
