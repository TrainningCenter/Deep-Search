param(
    [Parameter(Mandatory = $true)]
    [string]$UserName,

    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [switch]$EnableAuthenticatedNetworkShares
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

# Install the matching desktop binary and a Start Menu shortcut when the
# release package contains both executables beside one another.
$uiBinary = Join-Path (Split-Path -Parent $resolvedBinary) "deep-search-ui.exe"
if (Test-Path -LiteralPath $uiBinary -PathType Leaf) {
    $installedUi = Join-Path $installDirectory "deep-search-ui.exe"
    Copy-Item -LiteralPath $uiBinary -Destination $installedUi -Force
    $startMenu = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs"
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $shortcutPath = Join-Path $startMenu "Deep-Search.lnk"
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $installedUi
    $shortcut.WorkingDirectory = $installDirectory
    $shortcut.Description = "Deep-Search file indexing and search"
    $shortcut.IconLocation = "$installedUi,0"
    $shortcut.Save()
}
$grant = "${UserName}:(OI)(CI)M"
& icacls.exe $dataDirectory /inheritance:r /grant:r $grant "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not grant the selected account access to the Deep-Search data directory."
}

$arguments = 'serve "' + $databasePath + '" 127.0.0.1:4765'
$taskAction = New-ScheduledTaskAction -Execute $installedBinary -Argument $arguments -WorkingDirectory $installDirectory
$taskTrigger = New-ScheduledTaskTrigger -AtStartup
$taskSettings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 5 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries

if ($EnableAuthenticatedNetworkShares) {
    # Task Scheduler stores this credential using Windows-protected task credentials.
    # Password logon allows the background task to authenticate to UNC network shares.
    $credential = Get-Credential -UserName $UserName -Message "Enter the Windows account password for Deep-Search network-share access"
    if ($null -eq $credential) {
        throw "A Windows account credential is required when -EnableAuthenticatedNetworkShares is selected."
    }
    $password = $credential.GetNetworkCredential().Password
    $credentialGrant = "$($credential.UserName):(OI)(CI)M"
    & icacls.exe $dataDirectory /grant:r $credentialGrant | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Could not grant the credential account access to the Deep-Search data directory."
    }
    Register-ScheduledTask -TaskName "Deep-Search" -Action $taskAction -Trigger $taskTrigger -User $credential.UserName -Password $password -RunLevel Highest -Settings $taskSettings -Description "Deep-Search background indexing service" -Force | Out-Null
    Write-Host "Authenticated network-share access is enabled for the Deep-Search startup task."
} else {
    $taskPrincipal = New-ScheduledTaskPrincipal -UserId $UserName -LogonType S4U -RunLevel Highest
    Register-ScheduledTask -TaskName "Deep-Search" -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Settings $taskSettings -Description "Deep-Search background indexing service" -Force | Out-Null
    Write-Warning "S4U mode does not store a password and cannot authenticate to protected network shares. Use -EnableAuthenticatedNetworkShares to configure password-backed task logon for UNC paths."
}

Start-ScheduledTask -TaskName "Deep-Search"
Write-Host "Deep-Search is installed and scheduled to start at system boot for $UserName."
