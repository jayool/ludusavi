# Script para desinstalar ludusavi-daemon

$TaskName = "LudusaviDaemon"

$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $existing) {
    Write-Host "Task not found: $TaskName"
    exit 0
}

Write-Host "Stopping task..."
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue

Write-Host "Removing task..."
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false

Write-Host "Done. Ludusavi daemon has been uninstalled."
