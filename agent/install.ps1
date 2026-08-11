param(
  [Parameter(Position = 0)][ValidateSet("install", "uninstall", "status")][string]$Action = "install",
  [string]$ServerId,
  [string]$Token,
  [string]$Url,
  [ValidateRange(15, 3600)][int]$Interval = 60,
  [ValidateRange(2, 60)][int]$CollectInterval = 5
)

$ErrorActionPreference = "Stop"
$TaskName = "NodeFlare Agent"
$InstallDir = Join-Path $env:ProgramData "NodeFlare"
$AgentFile = Join-Path $InstallDir "nodeflare-agent.exe"
$RunnerFile = Join-Path $InstallDir "run.ps1"

function Assert-Safe([string]$Name, [string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9_./:@-]+$') {
    throw "Invalid $Name"
  }
}

function Assert-WorkerUrl([string]$Value) {
  $Parsed = $null
  $SecureScheme = $false
  if ([Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$Parsed)) {
    $SecureScheme = $Parsed.Scheme -eq "https" -or (
      $Parsed.Scheme -eq "http" -and $Parsed.Host -in @("localhost", "127.0.0.1", "::1")
    )
  }
  if (
    $Value.Length -gt 2048 -or
    $Value -match "\s" -or
    $Value.Contains("'") -or
    -not $SecureScheme -or
    -not [string]::IsNullOrEmpty($Parsed.UserInfo) -or
    -not [string]::IsNullOrEmpty($Parsed.Query) -or
    -not [string]::IsNullOrEmpty($Parsed.Fragment)
  ) {
    throw "Url must use HTTPS; HTTP is only allowed for loopback development"
  }
}

if ($Action -eq "uninstall") {
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "NodeFlare Agent removed."
  exit 0
}

if ($Action -eq "status") {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task) {
    Write-Error "NodeFlare Agent is not installed."
    exit 1
  }
  $Task
  exit 0
}

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "Run PowerShell as Administrator"
}
$NativeArchitecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($NativeArchitecture -ne "AMD64") {
  throw "Only Windows x86_64 is supported"
}
Assert-Safe "ServerId" $ServerId
Assert-Safe "Token" $Token
Assert-WorkerUrl $Url
$Url = $Url.TrimEnd('/')
$CollectInterval = [Math]::Min($CollectInterval, $Interval)

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
$Temporary = "$AgentFile.$PID.download.exe"
$ReleaseBase = "https://github.com/imengying/NodeFlare/releases/latest/download"
$Artifact = "agent-windows-x86_64.exe"
$ReleaseDownload = "$ReleaseBase/$Artifact"
try {
  Invoke-WebRequest -Uri $ReleaseDownload -OutFile $Temporary -TimeoutSec 120
  $Checksums = (Invoke-WebRequest -Uri "$ReleaseBase/SHA256SUMS" -TimeoutSec 30).Content
  $ChecksumMatch = [regex]::Match($Checksums, "(?m)^([0-9a-fA-F]{64})\s+\*?agent-windows-x86_64\.exe\s*$")
  if (-not $ChecksumMatch.Success) {
    throw "SHA256SUMS does not contain $Artifact"
  }
  $ExpectedChecksum = $ChecksumMatch.Groups[1].Value
  $ActualChecksum = (Get-FileHash -LiteralPath $Temporary -Algorithm SHA256).Hash
  if ($ActualChecksum -ne $ExpectedChecksum) {
    throw "Agent checksum verification failed"
  }
  & $Temporary version | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "Downloaded Agent failed its version check"
  }
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Move-Item -LiteralPath $Temporary -Destination $AgentFile -Force
} finally {
  Remove-Item -LiteralPath $Temporary -Force -ErrorAction SilentlyContinue
}

@"
`$env:SERVER_ID='$ServerId'
`$env:AGENT_TOKEN='$Token'
`$env:WORKER_URL='$Url'
`$env:REPORT_INTERVAL='$Interval'
`$env:COLLECT_INTERVAL='$CollectInterval'
& '$AgentFile' run
"@ | Set-Content -LiteralPath $RunnerFile -Encoding UTF8

$TaskAction = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$RunnerFile`""
$Trigger = New-ScheduledTaskTrigger -AtStartup
$Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
$Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $TaskName -Action $TaskAction -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "NodeFlare Agent installed and started."
