-- Backend Lua del plugin Ludusavi Sync.
--
-- Su único propósito en la Fase 1 es leer el token del daemon
-- (escrito por el daemon en %APPDATA%\ludusavi\daemon-token.txt) y
-- devolverlo al frontend. El frontend lo manda en el header
-- `Authorization: Bearer <token>` en cada request al daemon HTTP API.
--
-- Patrón validado en hello-world 4 (2026-05-06): Millennium IPC NO
-- serializa tablas Lua correctamente, sólo escalares. Por eso esta
-- función devuelve el token como string directo. Vacío significa
-- "no se pudo leer" (daemon nunca arrancado, fichero borrado, etc).

local logger = require("logger")
local millennium = require("millennium")

-- Path del token. En Windows %APPDATA%, en Linux $XDG_CONFIG_HOME (o
-- ~/.config como fallback). Mismo path que `app_dir()` resuelve en
-- el daemon Rust.
local function token_path()
    -- os.getenv("APPDATA") existe en Windows. Si está, usa ese path.
    local appdata = os.getenv("APPDATA")
    if appdata and appdata ~= "" then
        return appdata .. "\\ludusavi\\daemon-token.txt"
    end
    -- Fallback Linux/Mac: XDG_CONFIG_HOME o ~/.config
    local xdg = os.getenv("XDG_CONFIG_HOME")
    if not xdg or xdg == "" then
        local home = os.getenv("HOME") or ""
        xdg = home .. "/.config"
    end
    return xdg .. "/ludusavi/daemon-token.txt"
end

function read_daemon_token()
    local path = token_path()
    local file, err = io.open(path, "r")
    if not file then
        logger:error("[ludusavi-sync] cannot open token file " .. path .. ": " .. tostring(err))
        return ""
    end
    local content = file:read("*a")
    file:close()
    if not content then
        return ""
    end
    -- Strip whitespace
    content = content:gsub("^%s+", ""):gsub("%s+$", "")
    return content
end

-- Abre `args.path` con el explorador del SO. El frontend no puede
-- hacerlo por el sandbox CEF — tiene que delegar al backend Lua.
-- Devuelve "ok" si el comando se lanzó (no garantiza que el explorer
-- apareciera) o una string de error empezando por "error:".
--
-- IMPORTANTE — convenciones de Millennium IPC (validadas en
-- hello-world 4):
--  1. Args se reciben como tabla, no posicionales: el frontend
--     llama `openAppDirLua({ path: '...' })`, y aquí leemos
--     `args.path`. La signature posicional `function f(path)` da
--     error TS en el frontend.
--  2. El return value debe ser ESCALAR (string/number/bool). Si
--     devuelves tabla llega `undefined` al frontend sin error visible.
function open_app_dir(args)
    local path = args and args.path or nil
    if not path or path == "" then
        return "error: empty path"
    end
    -- Normaliza separadores en Windows. La GUI Iced almacena rutas
    -- con forward slashes (StrictPath::render); explorer.exe acepta
    -- ambos pero es más limpio normalizar.
    if package.config:sub(1, 1) == "\\" then
        path = path:gsub("/", "\\")
        -- explorer.exe "<path>" — si el path no existe, abre Mis
        -- Documentos, no falla.
        local cmd = string.format('start "" explorer "%s"', path)
        local ok = os.execute(cmd)
        if ok then
            return "ok"
        end
        return "error: explorer launch failed"
    else
        -- Linux: xdg-open. Mac: open. Probamos xdg-open primero.
        local ok = os.execute(string.format('xdg-open "%s" >/dev/null 2>&1', path))
        if ok then return "ok" end
        ok = os.execute(string.format('open "%s" >/dev/null 2>&1', path))
        if ok then return "ok" end
        return "error: no opener found (xdg-open / open)"
    end
end

local function on_load()
    logger:info("[ludusavi-sync] backend loaded")
    millennium.ready()
end

local function on_unload()
    logger:info("[ludusavi-sync] backend unloaded")
end

return {
    on_load = on_load,
    on_unload = on_unload,
}
