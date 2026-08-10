param(
  [Parameter(Position = 0)][ValidateSet("install", "uninstall", "status")][string]$Action = "install",
  [string]$ServerId,
  [string]$Token,
  [string]$Url,
  [ValidateRange(15, 3600)][int]$Interval = 60,
  [ValidateRange(2, 60)][int]$CollectInterval = 5
)

$ErrorActionPreference = "Stop"
$TaskName = "CF Monitor Agent"
$InstallDir = Join-Path $env:ProgramData "CFMonitor"
$AgentFile = Join-Path $InstallDir "cf-monitor-agent.exe"
$RunnerFile = Join-Path $InstallDir "run.ps1"

function Assert-Safe([string]$Name, [string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9_./:@-]+$') {
    throw "Invalid $Name"
  }
}

if ($Action -eq "uninstall") {
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "CF Monitor Agent removed."
  exit 0
}

if ($Action -eq "status") {
  Get-ScheduledTask -TaskName $TaskName
  exit 0
}

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "Run PowerShell as Administrator"
}
Assert-Safe "ServerId" $ServerId
Assert-Safe "Token" $Token
Assert-Safe "Url" $Url
$Url = $Url.TrimEnd('/')
$CollectInterval = [Math]::Min($CollectInterval, $Interval)

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
$Download = "https://github.com/imengying/CF-Monitor/releases/latest/download/agent-windows-x86_64.exe"
Invoke-WebRequest -Uri $Download -OutFile "$AgentFile.download"
& "$AgentFile.download" version | Out-Null
Move-Item -LiteralPath "$AgentFile.download" -Destination $AgentFile -Force

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
Write-Host "CF Monitor Agent installed and started."
