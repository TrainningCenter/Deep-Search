param(
    [Parameter(Mandatory = $true)]
    [string]$UserName,

    [Parameter(Mandatory = $true)]
    [string]$BinaryPath
)

$ErrorActionPreference = "Stop"
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principalCheck = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principalCheck.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this installer from an elevated PowerShell session."
}

$resolvedBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
if (-not (Test-Path -LiteralPath $resolvedBinary -PathType Leaf)) {
    throw "Deep-Search executable not found: $BinaryPath"
}

$installDirectory = Join-Path $env:ProgramFiles "Deep-Search"
$dataDirectory = Join-Path $env:ProgramData "Deep-Search"
$installedBinary = Join-Path $installDirectory "deep-search.exe"
$databasePath = Join-Path $dataDirectory "index.sqlite"

New-Item -ItemType Directory -Force -Path $installDirectory, $dataDirectory | Out-Null
Copy-Item -LiteralPath $resolvedBinary -Destination $installedBinary -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "deep-search-service.ps1") -Destination (Join-Path $installDirectory "deep-search-service.ps1") -Force
$grant = "${UserName}:(OI)(CI)M"
& icacls.exe $dataDirectory /inheritance:r /grant:r $grant "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not grant the selected account access to the Deep-Search data directory."
}

$arguments = 'serve "' + $databasePath + '" 127.0.0.1:4765'
$taskAction = New-ScheduledTaskAction -Execute $installedBinary -Argument $arguments -WorkingDirectory $installDirectory
$taskTrigger = New-ScheduledTaskTrigger -AtStartup
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $UserName -LogonType S4U -RunLevel Highest
$taskSettings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries

Register-ScheduledTask -TaskName "Deep-Search" -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Settings $taskSettings -Description "Deep-Search background indexing service" -Force | Out-Null
Start-ScheduledTask -TaskName "Deep-Search"

Write-Host "Deep-Search is installed and scheduled to start at system boot for $UserName."
Write-Warning "S4U scheduled tasks run without storing the user's password but cannot access authenticated network shares. Local and already-mounted storage can be indexed subject to account permissions."
