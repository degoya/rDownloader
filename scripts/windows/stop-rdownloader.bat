@echo off
rem Portable test stopper. Usage: stop-rdownloader.bat [all^|server^|capture]
rem A normal termination is attempted before remaining processes are killed after 10 seconds.
setlocal
cd /d "%~dp0"
set "MODE=%~1"
if not defined MODE set "MODE=all"
if /I not "%MODE%"=="all" if /I not "%MODE%"=="server" if /I not "%MODE%"=="capture" (
    echo Usage: %~nx0 [all^|server^|capture]
    exit /b 2
)

if /I "%MODE%"=="all" call :stop_process rdownloader-capture.exe
if /I "%MODE%"=="capture" call :stop_process rdownloader-capture.exe
if /I "%MODE%"=="all" call :stop_process rdownloader.exe
if /I "%MODE%"=="server" call :stop_process rdownloader.exe
exit /b 0

:stop_process
set "RD_STOP_EXE=%~dp0%~1"
set "RD_STOP_NAME=%~n1"
set "RD_STOP_PID=%~dp0run\%~n1.pid"
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'SilentlyContinue'; $exe = [IO.Path]::GetFullPath($env:RD_STOP_EXE); function Test-ExactProcess($candidate) { if ($null -eq $candidate) { return $false }; try { return [IO.Path]::GetFullPath($candidate.Path) -ieq $exe } catch { return $false } }; $process = $null; if (Test-Path -LiteralPath $env:RD_STOP_PID) { $stored = 0; if ([int]::TryParse(([IO.File]::ReadAllText($env:RD_STOP_PID).Trim()), [ref]$stored)) { $candidate = Get-Process -Id $stored -ErrorAction SilentlyContinue; if (Test-ExactProcess $candidate) { $process = $candidate } } }; if ($null -eq $process) { $process = Get-Process -Name $env:RD_STOP_NAME -ErrorAction SilentlyContinue | Where-Object { Test-ExactProcess $_ } | Select-Object -First 1 }; if ($null -eq $process) { Remove-Item -LiteralPath $env:RD_STOP_PID -Force -ErrorAction SilentlyContinue; Write-Output ($env:RD_STOP_NAME + '.exe is not running.'); exit 0 }; $processId = $process.Id; Start-Process -FilePath taskkill.exe -ArgumentList @('/PID', [string]$processId) -WindowStyle Hidden -Wait | Out-Null; for ($attempt = 0; $attempt -lt 10; $attempt++) { Start-Sleep -Seconds 1; $candidate = Get-Process -Id $processId -ErrorAction SilentlyContinue; if (-not (Test-ExactProcess $candidate)) { Remove-Item -LiteralPath $env:RD_STOP_PID -Force -ErrorAction SilentlyContinue; Write-Output ($env:RD_STOP_NAME + '.exe stopped.'); exit 0 } }; Start-Process -FilePath taskkill.exe -ArgumentList @('/F', '/PID', [string]$processId) -WindowStyle Hidden -Wait | Out-Null; Remove-Item -LiteralPath $env:RD_STOP_PID -Force -ErrorAction SilentlyContinue; Write-Output ($env:RD_STOP_NAME + '.exe terminated forcefully.')"
exit /b 0
