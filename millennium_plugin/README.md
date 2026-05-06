# Ludusavi Sync — Plugin Millennium (Fase 1)

Plugin de Steam Desktop (vía Millennium) para gestionar sync de saves
de Ludusavi sin salir del cliente. Implementa el plan
`MILLENNIUM_DECKY_DRAFT.md` tras validar técnicamente con los 4
hello-worlds de mayo 2026:

- HW1 ✓ — fetch a localhost desde plugin Millennium.
- HW4 ✓ — auth via token leído del filesystem por backend Lua.
- HW A ✓ — SSE long-lived sin desconexiones agresivas en CEF.
- HW B ✓ — `AddWindowCreateHook` permite inyecciones DOM contextuales.

## Estado de Fase 1

**MVP minimal entregado**: el plugin habla con el daemon HTTP y
valida la conexión. Sin pantallas de juegos / devices / settings
todavía — eso entra en commits sucesivos.

## Prerequisitos

- **Steam Desktop** + **Millennium** instalados.
- **Daemon Ludusavi** corriendo, **versión con HTTP API** (rama
  `claude/daemon-http-api` o un merge de ella). El daemon escribe el
  token a `%APPDATA%\ludusavi\daemon-token.txt` al primer arranque y
  expone `http://localhost:61234`.
- **Node.js LTS** + `npm` para compilar el plugin.

## Setup

```powershell
# 1. Instalar dependencias del plugin
npm install

# 2. Compilar
npm run build

# 3. Desplegar a Millennium (PowerShell normal, no admin)
$src = "C:\Users\jayoo\ludusavi\.claude\worktrees\peaceful-dubinsky-6a6d51\millennium_plugin"
$dst = "C:\Program Files (x86)\Steam\plugins\ludusavi-sync"
if (Test-Path $dst) { Remove-Item -Recurse -Force $dst }
robocopy $src $dst /S /XD node_modules /NFL /NDL /NJH /NJS

# 4. Reiniciar Steam (cerrar desde bandeja, no sólo la ventana).
# 5. Steam → Settings → Plugins → activar "Ludusavi Sync".
# 6. Abrir la pestaña del plugin desde Settings.
```

## Lo que ves al abrir la pestaña

Dos campos:

1. **Conexión con el daemon** — al cargar el plugin se ejecuta un
   `getStatus()` automático.
   - `✓ Conectado al daemon "ludusavi-daemon" v0.1.0 (api v1)` →
     todo funciona.
   - `✗ Daemon token unavailable…` → el daemon no está corriendo o
     no es la versión con HTTP. Botón Retry para reintentar tras
     arrancarlo.

2. **Cómo arreglar** (solo aparece en error) — checklist de tres
   cosas que verificar.

## Arquitectura

- `backend/main.lua`: lee `%APPDATA%\ludusavi\daemon-token.txt` y lo
  expone vía `read_daemon_token()` callable. Único punto de acceso
  al filesystem (el frontend no puede leer disco por sandbox CEF).
- `frontend/daemon-client.ts`: clase `DaemonClient` con métodos
  `getStatus()`, `getGames()`, `getDevices()`, `subscribeEvents()`.
  Cachea el token. Maneja 401 invalidando el cache para reintentar.
- `frontend/index.tsx`: el `definePlugin` con la pestaña y la
  pantalla mínima.

## Limitaciones conocidas (Fase 1)

- **SSE no funciona aún**. `EventSource` no acepta headers custom y
  el daemon de Fase 0 sólo valida el token via `Authorization`
  header. Hay que añadir auth via query string al daemon o usar
  `fetch` con streaming. Pendiente en commit posterior.
- **Read-only**. No hay aún forma de cambiar el modo de un juego
  desde el plugin — eso requiere endpoints `POST` en el daemon.
- **Sin pantallas**. La tabla de Games, This Device, All Devices,
  Settings ACCELA — todo eso son commits sucesivos.

## Próximos commits

1. Render de la tabla de Games (read-only) tirando de `getGames()`.
2. Pantallas This Device + All Devices (read-only).
3. Endpoints `POST` en daemon + selector de modo en plugin.
4. SSE wired + refresh automático en cambios.
5. Pestaña ACCELA reusando el bridge Python existente.
