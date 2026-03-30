# Script para desinstalar ludusavi-daemon
# Ejecutar como Administrador

$ServiceName = "ludusavi-daemon"

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if (-not $existing) {
    Write-Host "Service not found: $ServiceName"
    exit 0
}

Write-Host "Stopping service..."
Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue

Write-Host "Removing service..."
sc.exe delete $ServiceName

Write-Host "Done. Ludusavi daemon has been uninstalled."
