@echo off
rem Portable test launcher. Usage: start-rdownloader.bat [all^|server^|capture]
rem Both EXE files must be next to this script. Output goes below .\logs\ and is
rem rewritten on every start. On a failure the window stays open so the reason can be read.
setlocal
cd /d "%~dp0"
if not exist "logs" mkdir "logs"
if not exist "run" mkdir "run"

set "MODE=%~1"
if not defined MODE set "MODE=all"
if /I not "%MODE%"=="all" if /I not "%MODE%"=="server" if /I not "%MODE%"=="capture" (
    echo Usage: %~nx0 [all^|server^|capture]
    call :hold
    exit /b 2
)

rem Optional settings (program defaults, adjust as needed):
rem set RDOWNLOADER_LISTEN=127.0.0.1:8710
rem set RDOWNLOADER_DATABASE=data\rdownloader.sqlite3
rem set RDOWNLOADER_DOWNLOADS=downloads
rem set RUST_LOG=rdownloader=info,rd_=info

set "FAILED=0"
if /I "%MODE%"=="all" call :start_hidden rdownloader.exe serve rdownloader
if /I "%MODE%"=="all" if errorlevel 1 set "FAILED=1"
if /I "%MODE%"=="server" call :start_hidden rdownloader.exe serve rdownloader
if /I "%MODE%"=="server" if errorlevel 1 set "FAILED=1"
if /I "%MODE%"=="all" call :start_hidden rdownloader-capture.exe run rdownloader-capture
if /I "%MODE%"=="all" if errorlevel 1 set "FAILED=1"
if /I "%MODE%"=="capture" call :start_hidden rdownloader-capture.exe run rdownloader-capture
if /I "%MODE%"=="capture" if errorlevel 1 set "FAILED=1"

if /I not "%MODE%"=="capture" echo Web UI: http://localhost:8710
if /I not "%MODE%"=="capture" if "%FAILED%"=="0" call :open_browser_if_unconfigured
echo Logs: %~dp0logs
if not "%FAILED%"=="0" call :hold
exit /b %FAILED%

:hold
rem Only when the window was opened by double-clicking, because that is the case where closing
rem it takes the message with it. Started from an existing console the output stays visible, and
rem a pause would hang anything that runs this unattended -- RDOWNLOADER_NO_PAUSE=1 forces that.
if defined RDOWNLOADER_NO_PAUSE exit /b 0
echo %cmdcmdline% | find /i "%~nx0" >nul || exit /b 0
echo.
pause
exit /b 0

:open_browser_if_unconfigured
rem First run: once the server answers, open the browser so the setup wizard shows up.
set "RD_UI_ADDR=%RDOWNLOADER_LISTEN%"
if not defined RD_UI_ADDR set "RD_UI_ADDR=127.0.0.1:8710"
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'Stop'; $base = 'http://' + ($env:RD_UI_ADDR -replace '^0\.0\.0\.0', '127.0.0.1'); for ($i = 0; $i -lt 30; $i++) { try { $status = Invoke-RestMethod -Uri ($base + '/api/v1/auth/status') -TimeoutSec 2; if ($status.setup_required) { Start-Process $base }; exit 0 } catch { Start-Sleep -Milliseconds 500 } }"
exit /b 0

:start_hidden
rem %1 = EXE, %2 = argument, %3 = log name
if not exist "%~dp0%~1" (
    echo %~1 was not found: %~dp0%~1
    exit /b 1
)
set "RD_START_EXE=%~dp0%~1"
set "RD_START_ARG=%~2"
set "RD_START_CWD=%~dp0"
set "RD_START_OUT=%~dp0logs\%~3.log"
set "RD_START_ERR=%~dp0logs\%~3.err.log"
set "RD_START_PID=%~dp0run\%~3.pid"
rem Test-SamePath answers three ways on purpose: $true same executable, $false a different one,
rem and $null "could not tell" -- reading another process's path fails whenever this user may not
rem open it. Counting that third case as "not running" is what started a second agent on top of
rem the first, which then died on the Click'n'Load port and looked like a broken install.
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'Stop';" ^
  "function Test-SamePath($candidate, $target) { try { return [IO.Path]::GetFullPath($candidate.Path) -ieq $target } catch { return $null } };" ^
  "$exe = [IO.Path]::GetFullPath($env:RD_START_EXE); $name = [IO.Path]::GetFileNameWithoutExtension($exe);" ^
  "$candidates = @(Get-Process -Name $name -ErrorAction SilentlyContinue);" ^
  "$process = $candidates | Where-Object { (Test-SamePath $_ $exe) -eq $true } | Select-Object -First 1;" ^
  "if ($null -eq $process) { $process = $candidates | Where-Object { $null -eq (Test-SamePath $_ $exe) } | Select-Object -First 1 };" ^
  "if ($null -eq $process) { $process = Start-Process -FilePath $exe -ArgumentList $env:RD_START_ARG -WorkingDirectory $env:RD_START_CWD -WindowStyle Hidden -RedirectStandardOutput $env:RD_START_OUT -RedirectStandardError $env:RD_START_ERR -PassThru; Start-Sleep -Seconds 1; if ($process.HasExited) { exit $process.ExitCode } };" ^
  "[IO.File]::WriteAllText($env:RD_START_PID, [string]$process.Id)"
rem 10 means the capture agent has nothing to connect to yet, which is what a fresh install
rem looks like: pairing happens in the web interface once the server is up. Not a failure.
if errorlevel 10 if not errorlevel 11 (
    echo %~1 is not paired yet. Open the web interface, go to Settings ^> Desktop client,
    echo and run the command it shows. Then start this again.
    exit /b 0
)
rem 11 means a Click'n'Load listener already holds the port. Nothing but a capture agent binds
rem it, so this is "there is already one" rather than a fault.
if errorlevel 11 if not errorlevel 12 (
    echo %~1 did not start: another Click'n'Load listener already has port 9666.
    echo That is a second rdownloader-capture ^(check your autostart^) or JDownloader.
    echo Nothing to do -- the one that is running does the same job.
    exit /b 0
)
if errorlevel 1 (
    echo %~1 could not be started or exited during startup. See logs\%~3.err.log
    exit /b 1
)
echo %~1 is running -^> logs\%~3.log
exit /b 0
