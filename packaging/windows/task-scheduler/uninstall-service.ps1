$ErrorActionPreference = "Stop"
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principalCheck = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principalCheck.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Run this uninstaller from an elevated PowerShell session."
}

Stop-ScheduledTask -TaskName "Deep-Search" -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName "Deep-Search" -Confirm:$false -ErrorAction SilentlyContinue
$installDirectory = Join-Path $env:ProgramFiles "Deep-Search"
Remove-Item -LiteralPath $installDirectory -Recurse -Force -ErrorAction SilentlyContinue
$startMenuShortcut = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\Deep-Search.lnk"
Remove-Item -LiteralPath $startMenuShortcut -Force -ErrorAction SilentlyContinue
Write-Host "Deep-Search startup task removed. The index database in ProgramData was preserved."
