/**
 * Cliente HTTP para el daemon de Ludusavi Sync.
 *
 * Wraps las requests al daemon (`http://localhost:61234/api/*`) con
 * el token de auth. El token lo proporciona el backend Lua leyéndolo
 * del filesystem (`app_dir/daemon-token.txt`) — el frontend nunca lee
 * disco directamente porque el sandbox CEF no lo permite.
 *
 * Capa 0 del plan Millennium/Decky validada en hello-worlds 1 + 4 + A
 * el 2026-05-06.
 */

import { callable } from '@steambrew/client';

// ============================================================================
// Tipos del API (deben matchear la estructura serializada por
// src/sync/daemon_http.rs en el repo del fork).
// ============================================================================

export interface DaemonStatus {
  daemon: string;
  version: string;
  api_version: number;
  /** Path al app_dir de Ludusavi (p.ej. `C:/Users/.../AppData/Roaming/ludusavi`).
   *  El plugin lo pasa al backend Lua para abrirlo con el explorador
   *  del SO. Sólo expuesto desde la versión que añade el card SYNC
   *  DAEMON al overlay. */
  app_dir?: string;
  /** Último mod_time global registrado por el worker (de
   *  daemon-state.json). Es el "Last sync" que la GUI muestra en
   *  This Device → SYNC DAEMON. Distinto del per-device
   *  last_sync_time_utc que devuelve /api/devices. */
  last_sync_time_utc?: string;
  /** Siempre true cuando se recibe la respuesta. Existe para que el
   *  plugin pueda renderizar el dot verde "Daemon is running" sin
   *  condicionales especiales. */
  running?: boolean;
}

/** Estado del binario rclone reportado por el daemon. */
export type RcloneState = 'ok' | 'missing' | 'not_configured';

export interface ApiCloudResponse {
  /** "Google Drive" / "Dropbox" / "OneDrive" / "Box" / "FTP" / "SMB"
   *  / "WebDAV" / "Custom" / "Not configured". */
  provider: string;
  /** rclone remote ID (p.ej. "ludusavi-1234567"). "—" si no hay
   *  remote configurado. */
  remote_id: string;
  /** Carpeta cloud para los backups (p.ej. "ludusavi-backup"). */
  path: string;
  rclone_state: RcloneState;
  /** URL para instalar rclone si está missing. */
  install_url: string;
  /** Path al binario rclone configurado en config.apps.rclone.path.
   *  Sólo expuesto desde la versión que añade /api/settings al
   *  daemon — usado en la card CLOUD/RCLONE de la pantalla Settings. */
  rclone_executable?: string;
  /** Flags globales que el daemon pasa a rclone (p.ej.
   *  "--fast-list --ignore-checksum"). Mismo motivo que arriba. */
  rclone_arguments?: string;
}

// ----------------------------------------------------------------------
// /api/settings — config completa relevante para Settings tab
// ----------------------------------------------------------------------

export interface ApiSettingsSecondaryManifestLocal {
  kind: 'local';
  path: string;
  enabled: boolean;
}

export interface ApiSettingsSecondaryManifestRemote {
  kind: 'remote';
  url: string;
  enabled: boolean;
}

export type ApiSettingsSecondaryManifest =
  | ApiSettingsSecondaryManifestLocal
  | ApiSettingsSecondaryManifestRemote;

export interface ApiSettingsManifest {
  primary_url: string;
  primary_enabled: boolean;
  secondary: ApiSettingsSecondaryManifest[];
}

export interface ApiSettingsRoot {
  /** Store en camelCase: steam, epic, gog, gogGalaxy, heroic,
   *  legendary, lutris, microsoft, origin, prime, uplay, otherHome,
   *  otherWine, otherWindows, otherLinux, otherMac, other. */
  store: string;
  path: string;
}

export interface ApiSettingsSafety {
  safety_backups_enabled: boolean;
  system_notifications_enabled: boolean;
}

export interface ApiSettingsService {
  /** True si la scheduled task / systemd service está instalada.
   *  Sólo Windows por ahora — Linux/Mac devuelven false. */
  installed: boolean;
}

export interface ApiSettingsResponse {
  backup_path: string;
  manifest: ApiSettingsManifest;
  roots: ApiSettingsRoot[];
  safety: ApiSettingsSafety;
  service: ApiSettingsService;
}

export interface ApiDevice {
  id: string;
  name: string;
}

export interface ApiErrorInfo {
  category: string;
  direction: string;
  message: string;
}

export interface ApiConflictInfo {
  local_time?: string;
  cloud_time?: string;
  cloud_from?: string;
}

export interface ApiGameRow {
  name: string;
  /** "none" | "local" | "cloud" | "sync" */
  mode: string;
  auto_sync: boolean;
  /** "synced" | "pending_backup" | "pending_restore" | "not_managed"
   *  | "error" | "conflict" | "" */
  status: string;
  registered_here: boolean;
  registered_elsewhere: boolean;
  save_path?: string;
  last_synced_from?: string;
  last_sync_time_utc?: string;
  latest_write_time_utc?: string;
  storage_bytes: number;
  error?: ApiErrorInfo;
  conflict?: ApiConflictInfo;
}

export interface ApiGamesResponse {
  device: ApiDevice;
  games: ApiGameRow[];
  device_names: Record<string, string>;
  rclone_missing: boolean;
}

export interface ApiDeviceRow {
  id: string;
  name: string;
  is_current: boolean;
  games: string[];
  last_sync_time_utc?: string;
}

export interface ApiDevicesResponse {
  current_device_id: string;
  devices: ApiDeviceRow[];
}

/** Una entrada de ACCELA installed games. Coincide con `ApiAccelaInstall`
 *  en daemon_http.rs. Estos juegos pueden NO aparecer en /api/games si
 *  Ludusavi todavía no los ha registrado para sync — son la 4ª fuente
 *  del game-list (junto con manifest, custom games y backups previos). */
export interface ApiAccelaInstall {
  appid: string;
  game_name: string;
  install_path: string;
  library_path: string;
  size_on_disk: number;
  buildid: string;
  last_updated: string;
  accela_marker_path: string;
  appmanifest_path: string;
}

export interface ApiAccelaInstallsResponse {
  installs: ApiAccelaInstall[];
  /** Mensaje opcional cuando devolvemos lista vacía: explica el motivo
   *  (no configurado, adapter no encontrado, error). El plugin lo puede
   *  surface al usuario como hint. */
  note?: string;
}

/** Eventos del SSE stream — coincide con `DaemonEvent` en daemon_http.rs. */
export type DaemonEvent =
  | { type: 'games_changed' }
  | { type: 'devices_changed' }
  | { type: 'daemon_restarted' };

// ============================================================================
// Cliente
// ============================================================================

const DAEMON_URL = 'http://localhost:61234';

/** Función Lua del backend que devuelve el token o "" si no se puede leer. */
const readDaemonToken = callable<[], string>('read_daemon_token');

/** Función Lua del backend que abre `path` con el explorador del SO.
 *  Devuelve "ok" si el comando se lanzó o "error: ..." si no. El
 *  frontend la necesita porque el sandbox CEF no permite abrir paths
 *  directamente desde JS (no `window.open('file://...')`, etc).
 *
 *  Millennium IPC pasa args como objeto (Record), no posicional —
 *  por eso aquí va `{ path }` y la función Lua recibe `args.path`. */
const openAppDirLua = callable<[{ path: string }], string>('open_app_dir');

/**
 * Wrapper conveniente para abrir el `app_dir` del daemon (devuelto por
 * /api/status). Devuelve true si se lanzó el comando (no garantiza
 * que el explorer apareciera) o false si falló.
 */
export async function openAppDir(path: string): Promise<boolean> {
  if (!path) return false;
  try {
    const result = await openAppDirLua({ path });
    if (result === 'ok') return true;
    console.warn('[ludusavi-sync] open_app_dir failed:', result);
    return false;
  } catch (e) {
    console.error('[ludusavi-sync] open_app_dir threw:', e);
    return false;
  }
}

export class DaemonClient {
  private tokenPromise: Promise<string> | null = null;

  /**
   * Cachea el token tras la primera llamada al backend Lua. Si la
   * lectura inicial devuelve vacío, no cachea — la siguiente llamada
   * vuelve a intentarlo (el daemon puede haber arrancado en el
   * intervalo).
   */
  private async getToken(): Promise<string> {
    if (this.tokenPromise) {
      const cached = await this.tokenPromise;
      if (cached) return cached;
      this.tokenPromise = null;
    }
    this.tokenPromise = (async () => {
      try {
        const result = await readDaemonToken();
        return typeof result === 'string' ? result : '';
      } catch (e) {
        console.error('[ludusavi-sync] readDaemonToken failed:', e);
        return '';
      }
    })();
    return await this.tokenPromise;
  }

  /** Resetea el token cacheado. Útil si el usuario reinicia el daemon
   *  y se regenera el fichero. */
  clearToken(): void {
    this.tokenPromise = null;
  }

  private async fetchJSON<T>(path: string): Promise<T> {
    const token = await this.getToken();
    if (!token) {
      throw new Error(
        'Daemon token unavailable — el daemon no está corriendo o nunca ha arrancado.',
      );
    }
    const res = await fetch(DAEMON_URL + path, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!res.ok) {
      // 401 = token rotó. Forzamos relectura para el próximo intento.
      if (res.status === 401) {
        this.clearToken();
      }
      throw new Error(`HTTP ${res.status} ${res.statusText} — ${await safeText(res)}`);
    }
    return (await res.json()) as T;
  }

  /** POST con body JSON — patrón para todos los write endpoints de
   *  Fase 2. Misma semántica de auth y errores que `fetchJSON`. */
  private async postJSON<TBody, TResp>(path: string, body: TBody): Promise<TResp> {
    const token = await this.getToken();
    if (!token) {
      throw new Error(
        'Daemon token unavailable — el daemon no está corriendo o nunca ha arrancado.',
      );
    }
    const res = await fetch(DAEMON_URL + path, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      if (res.status === 401) {
        this.clearToken();
      }
      throw new Error(
        `HTTP ${res.status} ${res.statusText} — ${await safeText(res)}`,
      );
    }
    return (await res.json()) as TResp;
  }

  getStatus(): Promise<DaemonStatus> {
    return this.fetchJSON('/api/status');
  }

  getGames(): Promise<ApiGamesResponse> {
    return this.fetchJSON('/api/games');
  }

  getDevices(): Promise<ApiDevicesResponse> {
    return this.fetchJSON('/api/devices');
  }

  /**
   * Lista los juegos detectados por ACCELA en disco. 4ª fuente del
   * game-list (junto con manifest, custom games, backups previos).
   *
   * El daemon spawna `python accela_adapter/adapter.py
   * --accela-path <bin>` y le manda `list_accela_installs`. Si ACCELA
   * no está configurado, devuelve `installs: []` con `note` explicando
   * el motivo — en ese caso el plugin simplemente no muestra estos
   * juegos extra (no es un error fatal).
   */
  getAccelaInstalls(): Promise<ApiAccelaInstallsResponse> {
    return this.fetchJSON('/api/accela-installs');
  }

  /** Config de cloud storage (provider, remote_id, path, rclone_state).
   *  Read-only en Fase 1 — el usuario configura cloud desde la GUI Iced. */
  getCloud(): Promise<ApiCloudResponse> {
    return this.fetchJSON('/api/cloud');
  }

  /** Config completa relevante para la pantalla Settings: backup_path,
   *  manifest, roots, safety toggles, service installed. Read-only en
   *  Fase 1 — la edición llega con POST endpoints en Fase 2. */
  getSettings(): Promise<ApiSettingsResponse> {
    return this.fetchJSON('/api/settings');
  }

  /**
   * Cambia los toggles SAFETY (safety_backups_enabled,
   * system_notifications_enabled). Ambos campos son opcionales — sólo
   * se actualiza lo que viene en el body. Devuelve el estado completo
   * tras el cambio (echo) para que el caller pueda reconciliar UI sin
   * re-fetch.
   *
   * Side-effect: el daemon rota `sync-games.json`, lo que dispara
   * `daemon_restarted` via SSE — la tab Settings refresca sola.
   *
   * Primer endpoint write de Fase 2; usa el patrón replicado en todos
   * los siguientes (POST + body opcional + echo).
   */
  setSafety(body: {
    safety_backups_enabled?: boolean;
    system_notifications_enabled?: boolean;
  }): Promise<ApiSettingsSafety> {
    return this.postJSON<typeof body, ApiSettingsSafety>(
      '/api/settings/safety',
      body,
    );
  }

  /**
   * Cambia el save mode de un juego. `mode` es uno de los 4 valores
   * del enum `SaveMode` del daemon, en wire format camelCase: 'none',
   * 'local', 'cloud', 'sync'.
   *
   * El daemon NO valida pre-condiciones (rclone, daemon corriendo).
   * Si el cliente quiere bloquear el cambio (p.ej. en Cloud sin
   * rclone), tiene que comprobarlo localmente antes de llamar.
   *
   * Side-effect: rota sync-games.json + emite SSE `daemon_restarted`.
   * El cliente refresca la tabla Games al recibir el evento.
   *
   * Devuelve echo con `name`, `mode`, y `auto_sync` (preservado del
   * estado previo) para que el caller reconcilie sin re-fetch.
   */
  setGameMode(
    name: string,
    mode: 'none' | 'local' | 'cloud' | 'sync',
  ): Promise<{ name: string; mode: string; auto_sync: boolean }> {
    return this.postJSON(
      `/api/games/${encodeURIComponent(name)}/mode`,
      { mode },
    );
  }

  /**
   * Toggle del flag auto_sync de un juego. `enabled=true` = el daemon
   * sincroniza el juego automáticamente al detectar cambios en disco;
   * false = sólo sync manual.
   *
   * Echo idéntico a setGameMode: `{name, mode, auto_sync}`. El cliente
   * reconcilia ambos campos sin re-fetch.
   *
   * No tiene sentido en mode=none (no se sync nada igualmente). El
   * cliente puede deshabilitar el toggle en ese caso para evitar
   * confusión.
   */
  setGameAutoSync(
    name: string,
    enabled: boolean,
  ): Promise<{ name: string; mode: string; auto_sync: boolean }> {
    return this.postJSON(
      `/api/games/${encodeURIComponent(name)}/auto-sync`,
      { enabled },
    );
  }

  /**
   * Suscribe al stream SSE. Devuelve el `EventSource` para que el
   * caller pueda cerrarlo cuando se desmonte el componente.
   *
   * EventSource (estándar W3C) NO acepta headers custom, así que el
   * token va en query string. El daemon valida tanto `Authorization:
   * Bearer` como `?token=...` (validado e2e desde 2026-05-06; ver
   * tests `status_returns_200_with_correct_query_token` y compañía).
   *
   * `onError` se llama cuando el browser detecta que la conexión se
   * cayó. EventSource reintenta automáticamente (con backoff propio del
   * navegador) — el caller normalmente no necesita hacer nada, pero el
   * callback existe para que pueda surface "reconectando..." al
   * usuario.
   */
  async subscribeEvents(
    onEvent: (event: DaemonEvent) => void,
    onError?: (e: Event) => void,
  ): Promise<EventSource> {
    const token = await this.getToken();
    if (!token) {
      throw new Error('Daemon token unavailable for SSE');
    }
    const url = `${DAEMON_URL}/api/events?token=${encodeURIComponent(token)}`;
    const es = new EventSource(url);
    es.onmessage = (e) => {
      try {
        const parsed = JSON.parse(e.data) as DaemonEvent;
        onEvent(parsed);
      } catch (err) {
        console.error('[ludusavi-sync] failed to parse SSE event:', err, e.data);
      }
    };
    if (onError) {
      es.onerror = onError;
    }
    return es;
  }
}

/** Singleton reusable por todo el plugin. */
export const daemon = new DaemonClient();

async function safeText(res: Response): Promise<string> {
  try {
    return await res.text();
  } catch {
    return '<no body>';
  }
}
