# The PowerShell steps of ci.yml's `rust` job on Windows: the launchers' argument checks, the
# debug executables, the portable launcher and the Scoop manifest's install (RD-1101-07 moved them
# out of the workflow as they were). The bash half is scripts/ci-platform-smoke.sh. CI only: each
# step calls it from the checkout root under `shell: pwsh`, whose $ErrorActionPreference `Stop`
# and closing `exit $LASTEXITCODE` apply here as they did inline.
#
#   ./scripts/ci-platform-smoke.ps1 launcher-args   # the launchers refuse a wrong mode
#   ./scripts/ci-platform-smoke.ps1 executables     # debug build, both executables' --version
#   ./scripts/ci-platform-smoke.ps1 portable        # portable launcher up to /api/v1/health and down
#   ./scripts/ci-platform-smoke.ps1 scoop-zip       # a release-layout zip of the debug build
#   ./scripts/ci-platform-smoke.ps1 scoop-install   # install, run, register and remove through Scoop
param([Parameter(Mandatory = $true)][string]$Step)

function Test-LauncherArgs {
    cmd /c scripts\windows\start-rdownloader.bat invalid-mode
    if ($LASTEXITCODE -ne 2) { throw "start script accepted an invalid mode" }
    cmd /c scripts\windows\stop-rdownloader.bat invalid-mode
    if ($LASTEXITCODE -ne 2) { throw "stop script accepted an invalid mode" }
    # The capture-only launcher reaches the capture agent and nothing else: without the EXE
    # next to it, it names rdownloader-capture.exe as missing and never the server.
    $out = cmd /c scripts\windows\start-capture.bat 2>&1 | Out-String
    if ($LASTEXITCODE -ne 1 -or $out -notmatch 'rdownloader-capture\.exe was not found' -or $out -match 'rdownloader\.exe was not found') { throw "capture launcher: $out" }
    # The runner ends a pwsh step with `exit $LASTEXITCODE`, which would turn the expected
    # refusal (2) into a failed step.
    exit 0
}

function Test-Executables {
    cargo build --locked -p rdownloader -p rd-capture
    & target/debug/rdownloader.exe --version
    & target/debug/rdownloader-capture.exe --version
}

function Test-PortableLauncher {
    $portableDir = Join-Path $env:RUNNER_TEMP "rdownloader-portable"
    New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
    Copy-Item target/debug/rdownloader.exe $portableDir
    Copy-Item scripts/windows/start-rdownloader.bat,scripts/windows/stop-rdownloader.bat $portableDir
    Push-Location $portableDir
    try {
        cmd /c start-rdownloader.bat server
        if ($LASTEXITCODE -ne 0) { throw "portable start script failed" }
        $healthy = $false
        for ($attempt = 0; $attempt -lt 10; $attempt++) {
            try {
                Invoke-RestMethod http://127.0.0.1:8710/api/v1/health | Out-Null
                $healthy = $true
                break
            } catch {
                Start-Sleep -Seconds 1
            }
        }
        if (-not $healthy) { throw "portable server health check failed" }
        # RD-180-02: the launcher stops the server over its API, not by ending the process.
        $stopped = cmd /c stop-rdownloader.bat server 2>&1 | Out-String
        if ($stopped -notmatch 'stopped gracefully') { throw "no graceful stop: $stopped" }
    } finally {
        cmd /c stop-rdownloader.bat server
    }
    Pop-Location
}

# RD-180-06: the Scoop manifest the same way as the Homebrew formula on macOS, over a zip in the
# release layout from this tree's debug build, served on loopback. The manifest between the two
# is rendered by scripts/ci-platform-smoke.sh scoop-manifest, because the generator is bash.
function New-ScoopZip {
    $fixture = Join-Path $env:RUNNER_TEMP 'scoop-fixture'
    $stage = Join-Path $fixture 'stage'
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item target/debug/rdownloader.exe,target/debug/rdownloader-capture.exe $stage
    Copy-Item scripts/windows/start-rdownloader.bat,scripts/windows/stop-rdownloader.bat,scripts/windows/start-capture.bat,scripts/windows/stop-capture.bat $stage
    Copy-Item LICENSE, README.md $stage
    Compress-Archive -Path "$stage/*" -DestinationPath (Join-Path $fixture 'rdownloader-windows-x86_64.zip')
}

function Test-ScoopInstall {
    $ErrorActionPreference = 'Stop'
    $fixture = Join-Path $env:RUNNER_TEMP 'scoop-fixture'
    $server = Start-Process python -ArgumentList '-m','http.server','8765','--bind','127.0.0.1' -WorkingDirectory $fixture -PassThru -WindowStyle Hidden
    try {
        Invoke-Expression "& {$(Invoke-RestMethod https://get.scoop.sh)} -RunAsAdmin"
        $env:PATH = "$env:USERPROFILE\scoop\shims;$env:PATH"
        scoop install (Join-Path $fixture 'out\rdownloader.json')
        if ($LASTEXITCODE -ne 0) { throw "scoop install failed" }
        $version = (rdownloader --version | Out-String)
        if ($version -notmatch [regex]::Escape((Get-Content (Join-Path $fixture 'out\rdownloader.json') | ConvertFrom-Json).version)) { throw "rdownloader --version: $version" }
        start-rdownloader server
        if ($LASTEXITCODE -ne 0) { throw "start-rdownloader failed" }
        try {
            $healthy = $false
            for ($attempt = 0; $attempt -lt 30; $attempt++) {
                try {
                    Invoke-RestMethod http://127.0.0.1:8710/api/v1/health | Out-Null
                    $healthy = $true
                    break
                } catch {
                    Start-Sleep -Seconds 1
                }
            }
            if (-not $healthy) { throw "the Scoop install's server did not answer" }
        } finally {
            stop-rdownloader server
        }
        # `persist`: the database is in Scoop's persist folder, where an update finds it.
        if (-not (Test-Path "$env:USERPROFILE\scoop\persist\rdownloader\data\rdownloader.sqlite3")) { throw "the database is not in the persist folder" }
        # The login entries and the URL handler store Scoop's `current` junction, not this
        # version's folder, which `scoop update` leaves behind and `scoop cleanup` deletes.
        # Started from the version folder: the shims already run the executables through
        # `current`, and the registration would pass without the mapping.
        $appVersion = (Get-Content (Join-Path $fixture 'out\rdownloader.json') | ConvertFrom-Json).version
        $versionDir = "$env:USERPROFILE\scoop\apps\rdownloader\$appVersion"
        $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
        # `autostart install` wants a paired agent; pairing stores the token and asks nobody.
        '0123456789abcdef0123456789abcdef' | & "$versionDir\rdownloader-capture.exe" configure --token-stdin
        if ($LASTEXITCODE -ne 0) { throw "rdownloader-capture configure failed" }
        foreach ($entry in @(@('rdownloader.exe', 'rDownloader Service'), @('rdownloader-capture.exe', 'rDownloader Capture'))) {
            $exe, $name = $entry
            & "$versionDir\$exe" autostart install
            if ($LASTEXITCODE -ne 0) { throw "$exe autostart install failed" }
            # The Run value starts a VBScript wrapper; the executable is in the .cmd beside it.
            $value = (Get-ItemProperty $runKey).$name
            if (-not ($value -match '"([^"]+\.vbs)"')) { throw "unexpected Run value ${name}: $value" }
            $wrapper = Get-Content ([IO.Path]::ChangeExtension($Matches[1], '.cmd')) -Raw
            if ($wrapper -notlike "*\apps\rdownloader\current\$exe*") { throw "$exe autostart registered a versioned path: $wrapper" }
            & "$versionDir\$exe" autostart remove
            if ($LASTEXITCODE -ne 0) { throw "$exe autostart remove failed" }
            if ($null -ne (Get-ItemProperty $runKey).$name) { throw "$exe autostart remove left the Run value" }
        }
        & "$versionDir\rdownloader-capture.exe" scheme install
        if ($LASTEXITCODE -ne 0) { throw "rdownloader-capture scheme install failed" }
        $handler = (Get-ItemProperty 'HKCU:\Software\Classes\rdownloader\shell\open\command').'(default)'
        if ($handler -notlike '*\apps\rdownloader\current\rdownloader-capture.exe*') { throw "the URL handler registered a versioned path: $handler" }
        & "$versionDir\rdownloader-capture.exe" scheme remove
        if ($LASTEXITCODE -ne 0) { throw "rdownloader-capture scheme remove failed" }
        scoop uninstall rdownloader
        if ($LASTEXITCODE -ne 0) { throw "scoop uninstall failed" }
    } finally {
        Stop-Process -Id $server.Id -ErrorAction SilentlyContinue
    }
    exit 0
}

switch ($Step) {
    'launcher-args' { Test-LauncherArgs }
    'executables' { Test-Executables }
    'portable' { Test-PortableLauncher }
    'scoop-zip' { New-ScoopZip }
    'scoop-install' { Test-ScoopInstall }
    default { throw "usage: scripts/ci-platform-smoke.ps1 launcher-args|executables|portable|scoop-zip|scoop-install" }
}
