param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("status", "is-active", "is-enabled", "start", "stop", "restart", "enable", "disable")]
    [string]$Action
)

$ErrorActionPreference = "Stop"
$taskName = "Deep-Search"
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction Stop

switch ($Action) {
    "status" {
        $task | Format-List TaskName, State, TaskPath
        Get-ScheduledTaskInfo -TaskName $taskName | Format-List LastRunTime, LastTaskResult, NextRunTime
    }
    "is-active" {
        if ($task.State -eq "Running") { "active" } else { "inactive" }
    }
    "is-enabled" {
        if ($task.State -eq "Disabled") { "disabled" } else { "enabled" }
    }
    "start" {
        Start-ScheduledTask -TaskName $taskName
    }
    "stop" {
        Stop-ScheduledTask -TaskName $taskName
    }
    "restart" {
        Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        Start-ScheduledTask -TaskName $taskName
    }
    "enable" {
        Enable-ScheduledTask -TaskName $taskName | Out-Null
    }
    "disable" {
        Disable-ScheduledTask -TaskName $taskName | Out-Null
    }
}
