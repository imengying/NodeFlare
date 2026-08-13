[CmdletBinding(DefaultParameterSetName = "Install")]
param(
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("t")][string]$Token,
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("e")][string]$Endpoint,
  [Parameter(ParameterSetName = "Install")][Alias("i")][ValidateRange(15, 3600)][int]$Interval = 60,
  [Parameter(ParameterSetName = "Uninstall", Mandatory = $true)][switch]$Uninstall,
  [Parameter(ParameterSetName = "Status", Mandatory = $true)][switch]$Status
)

$ErrorActionPreference = "Stop"
$TaskName = "NodeFlare Agent"
$InstallDir = Join-Path $env:ProgramData "NodeFlare"
$AgentFile = Join-Path $InstallDir "nodeflare-agent.exe"

function Assert-Safe([string]$Name, [string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9_./:@-]+$') {
    throw "Invalid $Name"
  }
}

function Assert-Endpoint([string]$Value) {
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
    throw "Endpoint must use HTTPS; HTTP is only allowed for loopback development"
  }
}

if ($Uninstall) {
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "NodeFlare Agent removed."
  exit 0
}

if ($Status) {
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
Assert-Safe "Token" $Token
Assert-Endpoint $Endpoint
$TokenLength = $Token.Length
if ($TokenLength -gt 512) {
  throw "Install argument is too long"
}
$Endpoint = $Endpoint.TrimEnd('/')

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
$Temporary = "$AgentFile.$PID.download.exe"
$ReleaseApi = "https://api.github.com/repos/imengying/NodeFlare/releases/latest"
$Artifact = "agent-windows-x86_64.exe"
try {
  $Release = Invoke-RestMethod -Uri $ReleaseApi -Headers @{ Accept = "application/vnd.github+json"; "User-Agent" = "nodeflare-installer" } -TimeoutSec 30
  $ReleaseAsset = $Release.assets | Where-Object { $_.name -eq $Artifact } | Select-Object -First 1
  if ($Release.tag_name -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$' -or $null -eq $ReleaseAsset) {
    throw "GitHub latest release is invalid or does not contain $Artifact"
  }
  $DigestMatch = [regex]::Match([string]$ReleaseAsset.digest, '^sha256:([0-9a-fA-F]{64})$')
  if (-not $DigestMatch.Success) {
    throw "GitHub release does not contain a SHA-256 digest for $Artifact"
  }
  $ExpectedChecksum = $DigestMatch.Groups[1].Value
  $DownloadUrl = "https://github.com/imengying/NodeFlare/releases/download/$($Release.tag_name)/$Artifact"
  Invoke-WebRequest -Uri $DownloadUrl -OutFile $Temporary -TimeoutSec 120
  $ActualChecksum = (Get-FileHash -LiteralPath $Temporary -Algorithm SHA256).Hash
  if ($ActualChecksum -ne $ExpectedChecksum) {
    throw "Agent checksum verification failed"
  }
  $InstalledVersion = (& $Temporary --version | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) {
    throw "Downloaded Agent failed its version check"
  }
  $InstalledVersion = ($InstalledVersion -split '\s+')[-1]
  $ExpectedVersion = $Release.tag_name.Substring(1)
  if ($InstalledVersion -ne $ExpectedVersion) {
    throw "Release $($Release.tag_name) contains Agent version $InstalledVersion"
  }
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Move-Item -LiteralPath $Temporary -Destination $AgentFile -Force
} finally {
  Remove-Item -LiteralPath $Temporary -Force -ErrorAction SilentlyContinue
}

$TaskArguments = "-e $Endpoint -t $Token -i $Interval"
$TaskAction = New-ScheduledTaskAction -Execute $AgentFile -Argument $TaskArguments
$Trigger = New-ScheduledTaskTrigger -AtStartup
$Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
$Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $TaskName -Action $TaskAction -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
$Task = $null
for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
  Start-Sleep -Milliseconds 500
  $Task = Get-ScheduledTask -TaskName $TaskName
  if ($Task.State -eq "Running") { break }
}
if ($Task.State -ne "Running") {
  throw "NodeFlare Agent scheduled task failed to start (state: $($Task.State))"
}
Write-Host "NodeFlare Agent $InstalledVersion installed and started."
