# Script para instalar ludusavi-daemon como tarea programada
# No requiere permisos de administrador
# Ejecutar como usuario normal

param(
    [string]$ExePath = "$PSScriptRoot\ludusavi-daemon.exe"
)

$TaskName = "LudusaviDaemon"
$LogFile = "$env:APPDATA\ludusavi\daemon.log"

# Comprueba si el ejecutable existe
if (-not (Test-Path $ExePath)) {
    Write-Error "Executable not found: $ExePath"
    exit 1
}

# Crea el directorio de logs si no existe
New-Item -ItemType Directory -Force -Path (Split-Path $LogFile) | Out-Null

# Elimina la tarea si ya existe
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue

# Configura la acción — redirige stdout y stderr al log
$Action = New-ScheduledTaskAction `
    -Execute $ExePath

# Trigger: al iniciar sesión del usuario actual
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME

# Configuración: reiniciar si falla, ejecutar aunque no haya red todavía
$Settings = New-ScheduledTaskSettingsSet `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -MultipleInstances IgnoreNew `
    -RunOnlyIfNetworkAvailable $false

# Registra la tarea
Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Settings $Settings `
    -Description "Ludusavi Sync Daemon - syncs game saves automatically" `
    -RunLevel Limited `
    -Force

# Arranca la tarea ahora
Start-ScheduledTask -TaskName $TaskName

$task = Get-ScheduledTask -TaskName $TaskName
Write-Host "Task status: $($task.State)"
Write-Host ""
Write-Host "Done. Ludusavi daemon will start automatically at login."
Write-Host ""
Write-Host "Useful commands:"
Write-Host "  Start-ScheduledTask -TaskName $TaskName    # arrancar"
Write-Host "  Stop-ScheduledTask -TaskName $TaskName     # parar"
Write-Host "  Get-ScheduledTask -TaskName $TaskName      # ver estado"
