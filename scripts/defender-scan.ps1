# Microsoft Defender over the Windows files rDownloader ships (RD-1400-07): Defender quarantined
# rdownloader-capture.exe at a tester's as `Trojan:Win32/Bearfoos.A!ml`, a machine-learning
# verdict on an unsigned program. ci.yml's `rust` job scans the executables it built and, report
# only, the last releases' Windows assets; release.yml's `msi` job scans every .exe and .msi the
# release ships before `publish` runs. Windows only, elevated (MpCmdRun needs it; GitHub's
# runners are).
#
#   ./scripts/defender-scan.ps1 <file or directory>...           # fails on a threat
#   ./scripts/defender-scan.ps1 -Report <file or directory>...   # prints the same, exits 0
#   ./scripts/defender-scan.ps1 -Report -Release v1.38.0,v1.40.0 # downloads the tags' Windows
#                                                                # zips and MSIs (gh), unpacks, scans
#   -SelfTest     scans an EICAR test file first; a scanner that misses it proves nothing (exit 3)
#   -EnableCloud  turns on cloud lookups (MAPS, never a sample) on this machine: ephemeral
#                 runners only, never a developer's own PC
#   -SkipUpdate   no signature update first
#
# A directory contributes every .exe, .msi, .dll and .zip below it; a file named is scanned as it
# is. Each file is scanned with `MpCmdRun.exe -Scan -ScanType 3 -File <f> -DisableRemediation`,
# which ignores Defender's exclusions (GitHub's image excludes C:\ and D:\), scans inside archives,
# quarantines nothing and prints its detections. MpCmdRun returns 0 for clean and 2 for a threat
# *or* a scan error, so a 2 without a detection in its output is an error here. On top of it,
# Get-MpThreatDetection is read for detections on those paths since the scan began.
#
# Exit codes: 0 every file clean (always with -Report); 1 a threat in at least one file;
# 3 nothing trustworthy to say: Defender missing or stopped, a scan error, a failed self-test.
#
# What this cannot see: GitHub's image switches off real-time protection, behaviour monitoring
# and cloud lookups (actions/runner-images, Configure-WindowsDefender.ps1). A scan here is
# static — signatures and the engine's local models, plus the cloud with -EnableCloud — and a
# clean result does not rule out a verdict a user's PC reaches when the program runs. The script
# prints the protection state before it scans.
# No positional binding: otherwise the first two paths would land in -Release and -Repository.
[CmdletBinding(PositionalBinding = $false)]
param(
    [switch]$Report,
    [switch]$SelfTest,
    [switch]$EnableCloud,
    [switch]$SkipUpdate,
    [string[]]$Release = @(),
    [string]$Repository = $env:GITHUB_REPOSITORY,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Path = @()
)

$ErrorActionPreference = 'Stop'

$WorkRoot = Join-Path $(if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }) 'defender-scan'
$Rows = [System.Collections.Generic.List[object]]::new()

# Exits with $code, or with 0 under -Report, which measures and never fails.
function Stop-Scan([int]$code) {
    if ($Report -and $code -ne 0) {
        Write-Host "report mode: exit $code turned into 0"
        exit 0
    }
    exit $code
}

# The newest MpCmdRun.exe: the platform folder Defender updates, else the one Windows brings.
function Find-MpCmdRun {
    $platform = Join-Path $env:ProgramData 'Microsoft\Windows Defender\Platform'
    if (Test-Path $platform) {
        $newest = Get-ChildItem $platform -Directory |
            Where-Object { Test-Path (Join-Path $_.FullName 'MpCmdRun.exe') } |
            Sort-Object { try { [version]($_.Name -replace '-.*$', '') } catch { [version]'0.0' } } -Descending |
            Select-Object -First 1
        if ($newest) { return Join-Path $newest.FullName 'MpCmdRun.exe' }
    }
    $inbox = Join-Path $env:ProgramFiles 'Windows Defender\MpCmdRun.exe'
    if (Test-Path $inbox) { return $inbox }
    return $null
}

function Write-State([string]$label, $value) {
    Write-Host ('  {0,-34} {1}' -f $label, $value)
}

function OnOff($enabled) { if ($enabled) { 'on' } else { 'OFF' } }

# Engine, signatures and which protection layers are on; plain words for what a scan then means.
function Show-DefenderState {
    $status = Get-MpComputerStatus
    $prefs = Get-MpPreference
    Write-Host 'Microsoft Defender on this machine:'
    Write-State 'running mode' $status.AMRunningMode
    Write-State 'antimalware client / engine' "$($status.AMProductVersion) / $($status.AMEngineVersion)"
    Write-State 'antivirus signatures' "$($status.AntivirusSignatureVersion) ($($status.AntivirusSignatureLastUpdated))"
    Write-State 'antivirus enabled' (OnOff $status.AntivirusEnabled)
    Write-State 'real-time protection' (OnOff $status.RealTimeProtectionEnabled)
    Write-State 'behaviour monitoring' (OnOff $status.BehaviorMonitorEnabled)
    Write-State 'scan of downloads (IOAV)' (OnOff $status.IoavProtectionEnabled)
    Write-State 'cloud lookups (MAPSReporting)' $(if ($prefs.MAPSReporting -gt 0) { "on ($($prefs.MAPSReporting))" } else { 'OFF (0)' })
    Write-State 'tamper protection' (OnOff $status.IsTamperProtected)
    if (-not $status.RealTimeProtectionEnabled -or -not $status.BehaviorMonitorEnabled -or $prefs.MAPSReporting -eq 0) {
        Write-Host ''
        Write-Host 'NOTE: real-time protection, behaviour monitoring or cloud lookups are off here (GitHub'
        Write-Host '      switches them off in its Windows images). This is a static scan: a clean result'
        Write-Host '      does not rule out a verdict a PC with every layer on reaches when the program runs.'
    }
    Write-Host ''
    return $status
}

function Enable-CloudLookups([string]$mpCmdRun) {
    Write-Host 'Turning on cloud lookups for this machine (MAPSReporting Advanced, samples never sent).'
    try {
        Set-MpPreference -MAPSReporting Advanced -SubmitSamplesConsent NeverSend
    } catch {
        Write-Host "::warning::could not turn on cloud lookups: $($_.Exception.Message)"
        return
    }
    $out = & $mpCmdRun -ValidateMapsConnection 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0) {
        Write-Host 'Cloud connection: validated.'
    } else {
        Write-Host "::warning::the cloud connection did not validate (exit $LASTEXITCODE): $($out.Trim())"
    }
}

function Update-Signatures([string]$mpCmdRun) {
    Write-Host 'Updating the Defender signatures ...'
    & $mpCmdRun -SignatureUpdate | Out-Host
    if ($LASTEXITCODE -eq 0) { return }
    Write-Host "the update through Windows Update ended with $LASTEXITCODE; trying Microsoft's servers directly"
    & $mpCmdRun -SignatureUpdate -MMPC | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "::warning::the signature update failed (exit $LASTEXITCODE); scanning with the signatures this machine has"
    }
}

# The files a list of paths stands for: a file itself, a directory's Windows binaries and archives.
function Expand-ScanPaths([string[]]$paths) {
    foreach ($p in $paths) {
        if (-not (Test-Path -LiteralPath $p)) { throw "no such file or directory: $p" }
        $item = Get-Item -LiteralPath $p
        if ($item.PSIsContainer) {
            Get-ChildItem -LiteralPath $item.FullName -Recurse -File |
                Where-Object { $_.Extension -in '.exe', '.msi', '.dll', '.zip' } |
                Sort-Object FullName |
                ForEach-Object { $_.FullName }
        } else {
            $item.FullName
        }
    }
}

# The `Threat : <name>` lines of MpCmdRun's "LIST OF DETECTED THREATS".
function Get-ThreatNames([string]$output) {
    [regex]::Matches($output, '(?m)^\s*Threat\s*:\s*(.+?)\s*$') | ForEach-Object { $_.Groups[1].Value }
}

# One file through MpCmdRun: Result 'clean', 'THREAT' or 'ERROR', with the names or the reason.
function Invoke-FileScan([string]$mpCmdRun, [string]$file) {
    $output = & $mpCmdRun -Scan -ScanType 3 -File $file -DisableRemediation 2>&1 | Out-String
    $code = $LASTEXITCODE
    $names = @(Get-ThreatNames $output)
    if ($names.Count -gt 0) { return [pscustomobject]@{ Result = 'THREAT'; Detail = ($names -join ', ') } }
    if ($code -eq 0) { return [pscustomobject]@{ Result = 'clean'; Detail = '' } }
    # "Scanning <file> found <n> threats." without the list: still a threat, its name unknown.
    $found = [regex]::Match($output, 'found (\d+) threats')
    if ($found.Success -and [int]$found.Groups[1].Value -gt 0) {
        return [pscustomobject]@{ Result = 'THREAT'; Detail = "$($found.Groups[1].Value) threat(s), name not printed" }
    }
    $last = ($output.Trim() -split "`r?`n" | Select-Object -Last 1)
    return [pscustomobject]@{ Result = 'ERROR'; Detail = "MpCmdRun exit $code, no detection listed: $last" }
}

# Detections Defender recorded on its own since $since for $file (real-time, IOAV), by name.
function Get-RecordedThreats([string]$file, [datetime]$since) {
    $detections = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object {
            $_.InitialDetectionTime -ge $since -and (@($_.Resources) -match [regex]::Escape($file))
        })
    foreach ($d in $detections) {
        $threat = Get-MpThreat -ThreatID $d.ThreatID -ErrorAction SilentlyContinue
        if ($threat) { $threat.ThreatName } else { "threat id $($d.ThreatID)" }
    }
}

# The EICAR test file, assembled at run time so this script is not itself a test file.
function Test-Scanner([string]$mpCmdRun) {
    $dir = Join-Path $WorkRoot 'self-test'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $eicar = Join-Path $dir 'eicar.com'
    $text = 'X5O!P%@AP[4\PZX54(P^)7CC)7}$' + 'EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*'
    [IO.File]::WriteAllText($eicar, $text, [Text.Encoding]::ASCII)
    $result = Invoke-FileScan $mpCmdRun $eicar
    Remove-Item -LiteralPath $eicar -Force -ErrorAction SilentlyContinue
    if ($result.Result -ne 'THREAT') {
        Write-Host "::error::self-test: Defender did not report the EICAR test file ($($result.Result) $($result.Detail)); a clean result would mean nothing"
        Stop-Scan 3
    }
    Write-Host "self-test: the EICAR test file is reported as $($result.Detail)"
}

# The Windows zips and MSIs of a release, unpacked beside them: zips expanded, MSIs through an
# administrative install (`msiexec /a`, which writes the files out and registers nothing).
function Get-ReleaseFiles([string]$tag) {
    if (-not $Repository) { throw '-Release needs -Repository <owner/name> or GITHUB_REPOSITORY' }
    $dir = Join-Path $WorkRoot "release-$tag"
    if (Test-Path $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    # The plain-named (English) MSIs only: the other languages hold the same files.
    & gh release download $tag --repo $Repository --dir $dir -p '*windows-x86_64.zip' -p '*windows-x86_64.msi' | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "gh release download $tag failed (exit $LASTEXITCODE)" }
    foreach ($zip in Get-ChildItem -LiteralPath $dir -Filter '*.zip' -File) {
        Expand-Archive -LiteralPath $zip.FullName -DestinationPath (Join-Path $dir "$($zip.BaseName).unzipped")
    }
    foreach ($msi in Get-ChildItem -LiteralPath $dir -Filter '*.msi' -File) {
        $target = Join-Path $dir "$($msi.BaseName).msi-files"
        $proc = Start-Process msiexec.exe -Wait -PassThru -ArgumentList @('/a', "`"$($msi.FullName)`"", '/qn', "TARGETDIR=`"$target`"")
        if ($proc.ExitCode -ne 0) { throw "msiexec /a $($msi.Name) ended with $($proc.ExitCode)" }
    }
    return $dir
}

function Add-Row([string]$source, [string]$file, [string]$result, [string]$detail) {
    $Rows.Add([pscustomobject]@{ Source = $source; File = $file; Result = $result; Detail = $detail })
    $line = if ($detail) { '{0,-7} {1}  {2}' -f $result, $detail, $file } else { '{0,-7} {1}' -f $result, $file }
    Write-Host $line
    if ($result -eq 'THREAT') {
        $kind = if ($Report) { 'warning' } else { 'error' }
        Write-Host "::${kind}::Defender: $detail in $file"
    }
}

# The table of every file in the job summary, with the runner and the signature version: the
# record a job file quotes.
function Write-Summary($status) {
    if (-not $env:GITHUB_STEP_SUMMARY) { return }
    $md = [System.Collections.Generic.List[string]]::new()
    $md.Add("### Microsoft Defender scan$(if ($Report) { ' (report only)' })")
    $md.Add('')
    $md.Add("Runner $env:ImageOS $env:ImageVersion; engine $($status.AMEngineVersion), signatures $($status.AntivirusSignatureVersion); real-time protection $(OnOff $status.RealTimeProtectionEnabled).")
    $md.Add('')
    $md.Add('| Source | File | Result |')
    $md.Add('| --- | --- | --- |')
    foreach ($r in $Rows) {
        $md.Add("| $($r.Source) | $(Split-Path -Leaf $r.File) | $($r.Result) $($r.Detail) |")
    }
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $md -Encoding utf8
}

# --- main -----------------------------------------------------------------------------------

if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) {
    Write-Host '::error::Microsoft Defender scans run on Windows only'
    Stop-Scan 3
}
if ($Path.Count -eq 0 -and $Release.Count -eq 0) {
    Write-Host 'usage: defender-scan.ps1 [-Report] [-SelfTest] [-EnableCloud] [-SkipUpdate] [-Release <tag>,...] <file or directory>...'
    exit 3
}

$mpCmdRun = Find-MpCmdRun
if (-not $mpCmdRun) {
    Write-Host '::error::MpCmdRun.exe not found: Microsoft Defender is not installed here'
    Stop-Scan 3
}
Write-Host "MpCmdRun: $mpCmdRun"
try {
    if ($EnableCloud) { Enable-CloudLookups $mpCmdRun }
    if (-not $SkipUpdate) { Update-Signatures $mpCmdRun }
    $status = Show-DefenderState
} catch {
    Write-Host "::error::Microsoft Defender does not answer: $($_.Exception.Message)"
    Stop-Scan 3
}
if (-not $status.AntivirusEnabled) {
    Write-Host '::error::Microsoft Defender Antivirus is disabled here; nothing it says would count'
    Stop-Scan 3
}
if ($SelfTest) { Test-Scanner $mpCmdRun }

$sources = [System.Collections.Generic.List[object]]::new()
foreach ($p in $Path) { $sources.Add([pscustomobject]@{ Name = 'this build'; Paths = @($p) }) }
foreach ($tag in $Release) {
    try {
        $sources.Add([pscustomobject]@{ Name = $tag; Paths = @(Get-ReleaseFiles $tag) })
    } catch {
        Add-Row $tag $tag 'ERROR' $_.Exception.Message
    }
}

$started = Get-Date
foreach ($source in $sources) {
    try {
        $files = @(Expand-ScanPaths $source.Paths)
    } catch {
        Add-Row $source.Name "$($source.Paths)" 'ERROR' $_.Exception.Message
        continue
    }
    foreach ($file in $files) {
        $scan = Invoke-FileScan $mpCmdRun $file
        $recorded = @(Get-RecordedThreats $file $started)
        if ($recorded.Count -gt 0 -and $scan.Result -ne 'THREAT') {
            $scan = [pscustomobject]@{ Result = 'THREAT'; Detail = ($recorded -join ', ') }
        }
        Add-Row $source.Name $file $scan.Result $scan.Detail
    }
}
Write-Summary $status

$threats = @($Rows | Where-Object Result -eq 'THREAT').Count
$errors = @($Rows | Where-Object Result -eq 'ERROR').Count
Write-Host ''
Write-Host "$($Rows.Count) files scanned: $($Rows.Count - $threats - $errors) clean, $threats with a threat, $errors not scanned"
if ($Rows.Count -eq 0) {
    Write-Host '::error::no file to scan'
    Stop-Scan 3
}
if ($threats -gt 0) { Stop-Scan 1 }
if ($errors -gt 0) { Stop-Scan 3 }
exit 0
