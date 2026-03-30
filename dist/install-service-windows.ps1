# Script para instalar ludusavi-daemon como servicio de Windows
# Equivalente a UseWindowsService() de EmuSync
# Ejecutar como Administrador

param(
    [string]$ExePath = "$PSScriptRoot\ludusavi-daemon.exe"
)

$ServiceName = "ludusavi-daemon"
$DisplayName = "Ludusavi Sync Daemon"
$Description = "Automatically syncs game saves between devices using Ludusavi"

# Comprueba si el ejecutable existe
if (-not (Test-Path $ExePath)) {
    Write-Error "Executable not found: $ExePath"
    exit 1
}

# Para e elimina el servicio si ya existe
$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Stopping existing service..."
    Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
    Write-Host "Removing existing service..."
    sc.exe delete $ServiceName
    Start-Sleep -Seconds 2
}

# Instala el nuevo servicio
Write-Host "Installing service: $ServiceName"
New-Service `
    -Name $ServiceName `
    -DisplayName $DisplayName `
    -Description $Description `
    -BinaryPathName $ExePath `
    -StartupType Automatic `
    -ErrorAction Stop

# Arranca el servicio
Write-Host "Starting service..."
Start-Service -Name $ServiceName

$service = Get-Service -Name $ServiceName
Write-Host "Service status: $($service.Status)"
Write-Host "Done. Ludusavi daemon will now start automatically with Windows."
