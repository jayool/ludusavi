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
