/**
 * DOM-probe del main window de Steam.
 *
 * Antes de inyectar nuestra pestaña SYNC necesitamos confirmar dónde
 * está la nav bar (Library / Store / Community / etc) y el content
 * area. Este probe se ejecuta a demanda desde la UI del plugin y
 * devuelve un report textual que el usuario puede ver y copiar
 * directamente en el panel — sin necesidad de DevTools.
 *
 * Patrón de acceso al main window basado en steam-browser-history.
 */

import { sleep } from '@steambrew/client';
import {
  ApiCloudResponse,
  ApiDeviceRow,
  ApiSettingsResponse,
  daemon,
  openAppDir,
} from './daemon-client';
import { describeGame, statusColor } from './game-format';
import { navigateToSyncRoute } from './sync-route';
import { relativeTime } from './time-format';


declare const g_PopupManager: any;
declare const Millennium: any;

const MAIN_WINDOW_NAME = 'SP Desktop_uid0';

interface ElementInfo {
  tag: string;
  id: string;
  classes: string[];
  text: string;
  rect: { x: number; y: number; w: number; h: number };
  ancestorPath: string[];
}

function summariseElement(el: Element, maxAncestors = 5): ElementInfo {
  const rect = el.getBoundingClientRect();
  const ancestorPath: string[] = [];
  let cur: Element | null = el.parentElement;
  let i = 0;
  while (cur && i < maxAncestors) {
    const cls =
      cur.className && typeof cur.className === 'string'
        ? cur.className.split(/\s+/).slice(0, 3).join('.')
        : '';
    ancestorPath.unshift(`${cur.tagName.toLowerCase()}${cls ? '.' + cls : ''}`);
    cur = cur.parentElement;
    i++;
  }
  return {
    tag: el.tagName.toLowerCase(),
    id: el.id || '',
    classes:
      typeof el.className === 'string' ? el.className.split(/\s+/).filter(Boolean) : [],
    text: (el.textContent || '').trim().slice(0, 60),
    rect: { x: rect.x, y: rect.y, w: rect.width, h: rect.height },
    ancestorPath,
  };
}

function fmt(info: ElementInfo): string {
  const cls = info.classes.length > 0 ? '.' + info.classes.slice(0, 4).join('.') : '';
  const id = info.id ? `#${info.id}` : '';
  return `<${info.tag}>${id}${cls} [${Math.round(info.rect.w)}x${Math.round(info.rect.h)}@${Math.round(info.rect.x)},${Math.round(info.rect.y)}]
    text="${info.text}"
    ancestors: ${info.ancestorPath.join(' > ')}`;
}

/**
 * Ejecuta el probe sobre el main window y devuelve un reporte
 * textual. Si el main window aún no está disponible, espera hasta
 * 10s. Devuelve un mensaje de error si no se puede acceder.
 */
export async function runDomProbe(): Promise<string> {
  const lines: string[] = [];
  const log = (s: string) => lines.push(s);

  log('=== DOM probe — main window de Steam ===');

  if (typeof g_PopupManager === 'undefined') {
    log('[ERROR] g_PopupManager no definido. Esta versión de Millennium no expone el global.');
    return lines.join('\n');
  }

  // Esperar al main window (hasta 10s).
  const deadline = Date.now() + 10000;
  let popup = g_PopupManager.GetExistingPopup(MAIN_WINDOW_NAME);
  while (!popup && Date.now() < deadline) {
    await sleep(200);
    popup = g_PopupManager.GetExistingPopup(MAIN_WINDOW_NAME);
  }
  if (!popup) {
    log('[ERROR] Main window no encontrado tras 10s. Nombres conocidos:');
    try {
      const all = g_PopupManager.m_mapPopups || g_PopupManager.GetPopups?.() || {};
      log(`  ${JSON.stringify(Object.keys(all))}`);
    } catch (e) {
      log(`  (no se pudo enumerar: ${e})`);
    }
    return lines.join('\n');
  }

  log(`✓ main window detectado: "${popup.m_strName}"`);
  const win = popup.m_popup;
  const doc: Document = win.document;
  log(`URL: ${win.location?.href || '<unknown>'}`);
  log(`title: ${doc.title}`);
  log(`viewport: ${win.innerWidth}x${win.innerHeight}`);
  log('');

  // 1. Búsqueda por texto: ¿qué elementos contienen "Biblioteca",
  //    "Tienda", "Comunidad" (la nav principal)?
  log('--- Nav text matches ---');
  const navTexts = ['Biblioteca', 'Tienda', 'Comunidad', 'Library', 'Store', 'Community'];
  for (const text of navTexts) {
    const all = doc.querySelectorAll('*');
    let match: Element | null = null;
    for (const el of Array.from(all)) {
      const t = el.textContent?.trim() || '';
      if (t.toLowerCase() === text.toLowerCase() && el.children.length === 0) {
        match = el;
        break;
      }
    }
    if (match) {
      log(`"${text}" → ${fmt(summariseElement(match, 6))}`);
    }
  }
  log('');

  // 2. Roles aria estándar.
  log('--- Aria roles ---');
  const roleSelectors = ['nav', '[role="navigation"]', '[role="tablist"]'];
  for (const sel of roleSelectors) {
    const matches = doc.querySelectorAll(sel);
    log(`${sel}: ${matches.length} matches`);
    Array.from(matches)
      .slice(0, 3)
      .forEach((el, i) => log(`  [${i}] ${fmt(summariseElement(el, 4))}`));
  }
  log('');

  // 3. Top-level body children — la layout principal.
  log('--- Top-level <body> children ---');
  Array.from(doc.body.children).forEach((el, i) => {
    const cls = typeof el.className === 'string' ? el.className.slice(0, 80) : '';
    const rect = el.getBoundingClientRect();
    log(`  [${i}] <${el.tagName.toLowerCase()}> "${cls}" ${Math.round(rect.width)}x${Math.round(rect.height)}`);
  });
  log('');

  // 4. Top 8 divs más grandes — candidatos a content area.
  log('--- Top 8 biggest divs (candidatos a content area) ---');
  const allDivs = Array.from(doc.querySelectorAll('div'));
  const candidates = allDivs
    .map((d) => ({ d, area: d.getBoundingClientRect().width * d.getBoundingClientRect().height }))
    .filter((c) => c.area > 50000)
    .sort((a, b) => b.area - a.area)
    .slice(0, 8);
  candidates.forEach((c, i) => {
    log(`[${i}] area=${Math.round(c.area)} ${fmt(summariseElement(c.d, 3))}`);
  });
  log('');

  // 5. Probar selectores conocidos / heurísticos.
  log('--- Selectores heurísticos ---');
  const knownSelectors = [
    '[class*="MainNav"]',
    '[class*="TopBar"]',
    '[class*="HeaderBar"]',
    '[class*="navbar"]',
    '[class*="FocusBar"]',
    '[class*="library_NavBar"]',
    '[class*="MainNavMenu"]',
    '[class*="MainHeader"]',
  ];
  for (const sel of knownSelectors) {
    const el = doc.querySelector(sel);
    log(`"${sel}": ${el ? fmt(summariseElement(el, 4)) : 'NO match'}`);
  }
  log('');

  // 6. Confirmación visual: poner un marker visible para que el
  //    usuario vea que el probe se ejecutó sobre el main window real.
  if (!doc.querySelector('[data-ludusavi-probe-done]')) {
    const marker = doc.createElement('div');
    marker.setAttribute('data-ludusavi-probe-done', '1');
    marker.textContent = '✓ Ludusavi probe ejecutado';
    marker.style.cssText = [
      'position: fixed',
      'bottom: 0',
      'right: 0',
      'background: #3ecf8e',
      'color: white',
      'z-index: 99999',
      'padding: 4px 10px',
      'font-family: "Motiva Sans", sans-serif',
      'font-size: 11px',
      'pointer-events: none',
    ].join(';');
    doc.body.appendChild(marker);
    setTimeout(() => marker.remove(), 5000);
    log('Marker visual injectado en main window por 5s.');
  }

  log('');
  log('=== fin del probe ===');
  return lines.join('\n');
}

/**
 * Inyecta un botón "SYNC" como sibling de Biblioteca/Tienda/Comunidad
 * en la nav principal de Steam. Localiza por TEXTO (las clases hash
 * cambian entre builds), lo que da resistencia a renombres.
 *
 * En esta versión el click sólo logguea — el handler real (mostrar
 * nuestra UI a pantalla completa) llega en la siguiente iteración
 * cuando confirmemos que el botón aparece bien.
 */
export async function injectTestSyncTab(): Promise<string> {
  return await injectSyncTabImpl(/* waitMs */ 0);
}

/**
 * Auto-inyección al cargar el plugin. Doble vía:
 *
 *  1. Polling inmediato durante hasta 30s — para el caso en que el
 *     plugin carga DESPUÉS de que Steam ya tenga el main window
 *     listo (reload del plugin, hot reload).
 *  2. `Millennium.AddWindowCreateHook` — para el caso del arranque
 *     en frío de Steam: cuando se cree cualquier ventana nueva
 *     reintentamos la inyección. Como es idempotente, sólo prende
 *     la vez que aparece el main window. Necesario porque al
 *     arrancar Steam el entrypoint del plugin puede ejecutarse
 *     ANTES de que `g_PopupManager` exista, en cuyo caso el polling
 *     de la vía 1 tampoco arranca.
 *
 * Pensado para llamarse desde el entrypoint del plugin sin
 * intervención del usuario.
 */
export function autoInjectSyncTabOnLoad(): void {
  // Vía 1: intento inmediato con polling.
  injectSyncTabImpl(/* waitMs */ 30000).then((result) => {
    console.log('[ludusavi-sync] auto-inject (polling):', result);
  });

  // Vía 2: hook de creación de ventanas. AddWindowCreateHook fue
  // validado en hello-world B (2026-05-06) — fire repetido en
  // muchas ventanas, lo aprovechamos como "trigger garantizado".
  // Idempotency check dentro de injectSyncTabImpl evita duplicar.
  if (typeof Millennium !== 'undefined' && Millennium.AddWindowCreateHook) {
    try {
      Millennium.AddWindowCreateHook(() => {
        injectSyncTabImpl(/* waitMs */ 5000).then((result) => {
          // Sólo logueamos si fue una inyección efectiva o un error
          // distinto a "ya inyectado" para no spamear la consola.
          if (result.startsWith('✓') || result.startsWith('[ERROR]')) {
            console.log('[ludusavi-sync] auto-inject (window-hook):', result);
          }
        });
      });
    } catch (e) {
      console.warn('[ludusavi-sync] AddWindowCreateHook failed:', e);
    }
  } else {
    console.warn('[ludusavi-sync] Millennium.AddWindowCreateHook no disponible');
  }
}

async function injectSyncTabImpl(waitMs: number): Promise<string> {
  const deadline = Date.now() + waitMs;

  // Espera a que `g_PopupManager` esté definido. Al arrancar Steam,
  // los plugins pueden cargarse antes de que ese global exista; sin
  // este loop salíamos por el `[ERROR] g_PopupManager no definido`
  // antes de tener oportunidad de inyectar (bug histórico).
  while (typeof g_PopupManager === 'undefined' && Date.now() < deadline) {
    await sleep(250);
  }
  if (typeof g_PopupManager === 'undefined') {
    return '[ERROR] g_PopupManager no definido (timeout)';
  }

  // Espera al main window.
  let popup = g_PopupManager.GetExistingPopup(MAIN_WINDOW_NAME);
  while (!popup && Date.now() < deadline) {
    await sleep(250);
    popup = g_PopupManager.GetExistingPopup(MAIN_WINDOW_NAME);
  }
  if (!popup) return '[ERROR] Main window no disponible (timeout)';
  const doc: Document = popup.m_popup.document;

  // Idempotente: si ya inyectado, no duplicamos.
  if (doc.querySelector('[data-ludusavi-sync-tab]')) {
    return 'Ya inyectado (sin cambios). Refresca Steam para retirarlo.';
  }

  // Localizar todos los textos de navegación por nombre (caja de
  // resistencia a build hashes / idiomas).
  function findLeafByText(text: string): Element | null {
    for (const el of Array.from(doc.querySelectorAll('div'))) {
      if (
        el.children.length === 0 &&
        el.textContent?.trim().toLowerCase() === text.toLowerCase()
      ) {
        return el;
      }
    }
    return null;
  }

  const biblioteca = findLeafByText('Biblioteca') || findLeafByText('Library');
  if (!biblioteca) {
    return '[ERROR] No se encontró el botón Biblioteca/Library en el main window';
  }
  const tienda = findLeafByText('Tienda') || findLeafByText('Store');
  if (!tienda) {
    return '[ERROR] No se encontró el botón Tienda/Store (necesario para localizar el row)';
  }

  // Walk-up: subimos por la cadena de ancestros del botón Biblioteca
  // hasta encontrar uno que TAMBIÉN contenga al botón Tienda. Ese es
  // el row real de la nav (y no un wrapper individual de Biblioteca).
  let row: Element | null = null;
  let walkup: Element | null = biblioteca.parentElement;
  let depth = 0;
  while (walkup && depth < 6) {
    if (walkup.contains(tienda)) {
      row = walkup;
      break;
    }
    walkup = walkup.parentElement;
    depth++;
  }
  if (!row) {
    return '[ERROR] No encontré ancestor común entre Biblioteca y Tienda en 6 niveles.';
  }

  // El ancestor inmediato del leaf "Biblioteca" es el wrapper del botón
  // (Steam parece envolver el texto en un div con estilo de tab). Lo
  // queremos clonar entero — no sólo el leaf — para heredar padding,
  // hover state, etc.
  let bibliotecaWrapper: Element = biblioteca;
  // Subimos UN nivel por debajo del row: ese es el wrapper del botón.
  while (bibliotecaWrapper.parentElement && bibliotecaWrapper.parentElement !== row) {
    bibliotecaWrapper = bibliotecaWrapper.parentElement;
  }

  // Clonar el wrapper, modificar el texto del leaf interno.
  const syncTab = bibliotecaWrapper.cloneNode(true) as HTMLElement;
  syncTab.setAttribute('data-ludusavi-sync-tab', '1');
  // Buscar el leaf de texto dentro del clon y cambiarlo.
  function changeText(el: Element, newText: string): boolean {
    if (el.children.length === 0) {
      el.textContent = newText;
      return true;
    }
    for (const child of Array.from(el.children)) {
      if (changeText(child, newText)) return true;
    }
    return false;
  }
  changeText(syncTab, 'SYNC');
  // Sin outline custom: el clon hereda el styling nativo de Steam,
  // queremos que se vea idéntico a Tienda/Comunidad. Activación
  // visual queda implícita (cuando el overlay está abierto, el
  // resto de la UI queda detrás).

  syncTab.addEventListener(
    'click',
    (e) => {
      e.stopPropagation();
      e.preventDefault();
      // Navegamos a la ruta personalizada `/ludusavi-sync` registrada
      // en sync-route.tsx. Steam renderizará nuestro componente en el
      // área de contenido principal, encima de Tienda/Comunidad/etc
      // — porque ya no es DOM nuestro pegado en main window, es un
      // route legítimo del router de Steam.
      navigateToSyncRoute();
    },
    true, // capture phase para llegar antes de los listeners de Steam
  );

  // Listeners en los OTROS tabs de la nav: cuando el usuario clica
  // Biblioteca/Tienda/Comunidad/etc, ocultamos nuestro overlay para
  // no taparles el contenido. Steam sigue procesando el click
  // normalmente porque NO usamos stopPropagation aquí.
  //
  // Cuando NUESTRO click handler (arriba) dispara click sintético
  // sobre Biblioteca, este listener también se dispara — pero
  // hideSyncOverlay es idempotente (no-op si el overlay no existe
  // o ya está oculto), así que la secuencia "click Biblioteca →
  // hide → showSyncOverlay" funciona correctamente.
  Array.from(row.children).forEach((sibling) => {
    if (sibling === syncTab) return;
    sibling.addEventListener('click', () => hideSyncOverlay(doc));
  });

  row.appendChild(syncTab);

  // Diagnóstico para debug si sigue sin verse bien.
  const childrenSummary = Array.from(row.children).map((c, i) => {
    const r = c.getBoundingClientRect();
    const text = (c.textContent || '').trim().slice(0, 25);
    return `[${i}] ${Math.round(r.width)}x${Math.round(r.height)}@${Math.round(r.x)},${Math.round(r.y)} "${text}"`;
  });

  return `✓ SYNC tab inyectada en el row real (depth=${depth} desde Biblioteca).
Row: ${fmt(summariseElement(row, 2))}
Wrapper clonado: ${fmt(summariseElement(bibliotecaWrapper, 1))}
Children del row tras inyectar (deberían estar todos a y≈34):
${childrenSummary.join('\n')}`;
}

const OVERLAY_ATTR = 'data-ludusavi-sync-overlay';

// Estado SSE a nivel de módulo: la conexión EventSource activa, y el
// timer de debounce. Nivel de módulo (no global window) porque el plugin
// se ejecuta en un único contexto JS — múltiples overlays simultáneos
// no es un caso de uso real.
let currentEventSource: EventSource | null = null;
let pendingRefreshTimer: ReturnType<typeof setTimeout> | null = null;

type SseStatus = 'connecting' | 'live' | 'reconnecting' | 'offline';

/** Tabs del overlay. Mismo modelo que las pantallas de la GUI Iced
 *  (Screen::Games, Screen::ThisDevice, Screen::AllDevices, Screen::Other). */
type ActiveTab = 'games' | 'this-device' | 'all-devices' | 'settings';

/** Tab activa entre llamadas a showSyncOverlay. Se preserva si el
 *  usuario abre/cierra el overlay sin reiniciar Steam. */
let currentTab: ActiveTab = 'games';

/** Filtro de búsqueda activo en la tab Games. Persiste entre cambios
 *  de tab (igual que en la GUI Iced). */
let gamesSearchQuery = '';

const TABS: { id: ActiveTab; label: string }[] = [
  { id: 'games', label: 'Games' },
  { id: 'this-device', label: 'This Device' },
  { id: 'all-devices', label: 'All Devices' },
  { id: 'settings', label: 'Settings' },
];

/** Renderiza la tab activa actual en el content area. */
export async function renderActiveTab(
  doc: Document,
  overlay: HTMLElement,
  content: HTMLElement,
) {
  applyTabStyling(overlay);
  switch (currentTab) {
    case 'games':
      await loadAndRenderGames(doc, content);
      break;
    case 'this-device':
      await loadAndRenderThisDevice(doc, content);
      break;
    case 'all-devices':
      await loadAndRenderAllDevices(doc, content);
      break;
    case 'settings':
      await loadAndRenderSettings(doc, content);
      break;
  }
}

/** Pinta la tab activa (subrayado azul) y las inactivas (gris). */
function applyTabStyling(overlay: HTMLElement) {
  const tabs = overlay.querySelectorAll<HTMLElement>('[data-tab-id]');
  tabs.forEach((tab) => {
    const id = tab.getAttribute('data-tab-id') as ActiveTab;
    const isActive = id === currentTab;
    tab.style.color = isActive ? '#ffffff' : '#9aa3b2';
    tab.style.borderBottomColor = isActive ? '#4f8ef7' : 'transparent';
    tab.style.fontWeight = isActive ? '600' : '400';
  });
}

/** Construye el "esqueleto" estático del overlay: header + content area. */
export function buildOverlayShell(doc: Document): HTMLElement {
  const overlay = doc.createElement('div');
  overlay.setAttribute(OVERLAY_ATTR, '1');
  overlay.style.cssText = [
    // Rellena el contenedor del modal de Steam (que ya provee
    // posicionamiento). Sin position:fixed/z-index porque el modal
    // está gestionado por Steam en su propio árbol React.
    'width: 100%',
    'height: 100%',
    'background: #0f1117',
    'color: #ffffff',
    'display: flex',
    'flex-direction: column',
    'padding: 24px 32px',
    'overflow: auto',
    'font-family: "Motiva Sans", sans-serif',
    'box-sizing: border-box',
  ].join(';');

  // Header con título + botón Refresh.
  const header = doc.createElement('div');
  header.style.cssText = [
    'display: flex',
    'align-items: center',
    'justify-content: space-between',
    'border-bottom: 1px solid #2a2f42',
    'padding-bottom: 12px',
    'margin-bottom: 16px',
  ].join(';');

  const title = doc.createElement('div');
  title.textContent = 'Ludusavi Sync';
  title.style.cssText = 'font-size: 20px; font-weight: 600;';
  header.appendChild(title);

  // Bloque derecho del header: status pill SSE + botón Refresh.
  const rightBlock = doc.createElement('div');
  rightBlock.style.cssText = 'display: flex; align-items: center; gap: 12px;';

  // Status pill: indicador del SSE stream. Empieza en 'connecting' y se
  // actualiza con eventos de la conexión.
  const statusPill = doc.createElement('div');
  statusPill.setAttribute('data-sse-status', 'connecting');
  statusPill.style.cssText = [
    'display: inline-flex',
    'align-items: center',
    'gap: 6px',
    'padding: 4px 10px',
    'border-radius: 12px',
    'background: #1f2433',
    'font-size: 11px',
    'color: #9aa3b2',
    'font-family: inherit',
  ].join(';');
  applyStatusPill(statusPill, 'connecting');
  rightBlock.appendChild(statusPill);

  const refreshBtn = doc.createElement('button');
  refreshBtn.textContent = 'Refresh';
  refreshBtn.style.cssText = [
    'background: #4f8ef7',
    'color: white',
    'border: none',
    'border-radius: 4px',
    'padding: 6px 14px',
    'font-size: 12px',
    'font-family: inherit',
    'cursor: pointer',
  ].join(';');
  refreshBtn.addEventListener('click', () => {
    const content = overlay.querySelector<HTMLElement>('[data-content]')!;
    renderActiveTab(doc, overlay, content);
  });
  rightBlock.appendChild(refreshBtn);

  header.appendChild(rightBlock);
  overlay.appendChild(header);

  // Tabs row: Games | This Device | All Devices.
  const tabsRow = doc.createElement('div');
  tabsRow.style.cssText = [
    'display: flex',
    'gap: 4px',
    'border-bottom: 1px solid #2a2f42',
    'margin-bottom: 16px',
  ].join(';');
  for (const tab of TABS) {
    const tabBtn = doc.createElement('button');
    tabBtn.setAttribute('data-tab-id', tab.id);
    tabBtn.textContent = tab.label;
    tabBtn.style.cssText = [
      'background: transparent',
      'border: none',
      'border-bottom: 2px solid transparent',
      'color: #9aa3b2',
      'padding: 8px 14px',
      'font-size: 13px',
      'font-family: inherit',
      'cursor: pointer',
      'margin-bottom: -1px', // overlap con el border del row
      'transition: color 0.15s',
    ].join(';');
    tabBtn.addEventListener('click', () => {
      if (currentTab === tab.id) return; // ya activa, no refetch
      currentTab = tab.id;
      const content = overlay.querySelector<HTMLElement>('[data-content]')!;
      renderActiveTab(doc, overlay, content);
    });
    tabsRow.appendChild(tabBtn);
  }
  overlay.appendChild(tabsRow);

  // Content area que se rellena async según la tab activa.
  const content = doc.createElement('div');
  content.setAttribute('data-content', '1');
  content.style.cssText = 'display: flex; flex-direction: column; gap: 6px;';
  overlay.appendChild(content);

  return overlay;
}

/**
 * Renderiza la tab "Games" en paridad con `Screen::Games` de la GUI:
 * search input + tabla con columnas explícitas (dot, NAME, MODE,
 * AUTO SYNC, LAST SYNCED FROM, LAST SYNCED).
 *
 * Sources: /api/games (3 fuentes Ludusavi: manifest + custom + backups
 * previos) + /api/accela-installs (4ª fuente). Merge por nombre.
 *
 * Botones write-mode (Scan now, + Add game, acciones por fila) llegan
 * en Fase 2 cuando expongamos los POST endpoints.
 */
async function loadAndRenderGames(doc: Document, content: HTMLElement) {
  content.innerHTML = '';
  const loading = doc.createElement('div');
  loading.textContent = 'Cargando...';
  loading.style.cssText = 'color: #9aa3b2; font-size: 13px; padding: 12px;';
  content.appendChild(loading);

  // Pedimos ambos endpoints en paralelo. Si /api/games falla es fatal
  // (no podemos mostrar nada). Si /api/accela-installs falla o devuelve
  // vacío con `note`, no es fatal — sólo no añadimos esos juegos.
  const [gamesSettled, accelaSettled] = await Promise.allSettled([
    daemon.getGames(),
    daemon.getAccelaInstalls(),
  ]);

  if (gamesSettled.status === 'rejected') {
    showFatalError(doc, content, gamesSettled.reason);
    return;
  }

  const resp = gamesSettled.value;
  content.innerHTML = '';

  // Set de nombres ya cubiertos por /api/games para deduplicar.
  const knownNames = new Set(resp.games.map((g) => g.name.toLowerCase()));

  // ACCELA installs que NO están ya en /api/games.
  let extraAccela: { name: string; install_path: string; size_on_disk: number }[] = [];
  let accelaNote: string | null = null;
  if (accelaSettled.status === 'fulfilled') {
    accelaNote = accelaSettled.value.note ?? null;
    extraAccela = accelaSettled.value.installs
      .filter((a) => !knownNames.has(a.game_name.toLowerCase()))
      .map((a) => ({
        name: a.game_name,
        install_path: a.install_path,
        size_on_disk: a.size_on_disk,
      }));
  } else {
    accelaNote = `ACCELA endpoint failed: ${accelaSettled.reason}`;
  }

  // Header summary.
  const summary = doc.createElement('div');
  const rcloneNote = resp.rclone_missing ? ' · ⚠ rclone unavailable' : '';
  const totalCount = resp.games.length + extraAccela.length;
  summary.textContent = `${totalCount} game(s) — device: ${resp.device.name}${rcloneNote}`;
  summary.style.cssText = 'color: #9aa3b2; font-size: 12px; margin-bottom: 12px;';
  content.appendChild(summary);

  // Note ACCELA si endpoint falló.
  if (accelaNote && extraAccela.length === 0) {
    const noteEl = doc.createElement('div');
    noteEl.textContent = `ℹ ${accelaNote}`;
    noteEl.style.cssText = 'color: #6b7280; font-size: 11px; margin-bottom: 8px;';
    content.appendChild(noteEl);
  }

  // Search input. Filtra por nombre. Persistente entre tab switches.
  const searchInput = doc.createElement('input');
  searchInput.type = 'text';
  searchInput.placeholder = 'Search games...';
  searchInput.value = gamesSearchQuery;
  searchInput.style.cssText = [
    'background: #1f2433',
    'color: #ffffff',
    'border: 1px solid #2a2f42',
    'border-radius: 4px',
    'padding: 8px 12px',
    'font-size: 13px',
    'font-family: inherit',
    'margin-bottom: 12px',
    'width: 100%',
    'box-sizing: border-box',
  ].join(';');
  // El listener actualiza el módulo state y re-renderiza solo el body
  // de la tabla — sin refetch al daemon (que sería overkill para un
  // filter de cliente).
  const tableBody = doc.createElement('div');
  tableBody.style.cssText = 'display: flex; flex-direction: column;';
  searchInput.addEventListener('input', () => {
    gamesSearchQuery = searchInput.value;
    redrawTableBody(doc, tableBody, resp, extraAccela);
  });
  content.appendChild(searchInput);

  if (totalCount === 0) {
    const empty = doc.createElement('div');
    empty.textContent = 'No games found. Run a backup scan first.';
    empty.style.cssText = 'color: #6b7280; padding: 24px; text-align: center;';
    content.appendChild(empty);
    return;
  }

  // Header de tabla: una row con labels en uppercase + columnas que
  // matcean con renderGameRow / renderAccelaOnlyRow.
  content.appendChild(buildTableHeader(doc));

  content.appendChild(tableBody);
  redrawTableBody(doc, tableBody, resp, extraAccela);
}

/** Re-renderiza el cuerpo de la tabla aplicando el filtro actual.
 *  Llamado al input del search (sin refetch). */
function redrawTableBody(
  doc: Document,
  tableBody: HTMLElement,
  resp: import('./daemon-client').ApiGamesResponse,
  extraAccela: { name: string; install_path: string; size_on_disk: number }[],
) {
  tableBody.innerHTML = '';
  const q = gamesSearchQuery.trim().toLowerCase();
  const matches = (n: string) => !q || n.toLowerCase().includes(q);

  // Para "LAST SYNCED FROM" necesitamos resolver device_id → device_name
  // del map que el daemon ya nos da en device_names.
  const resolveDeviceName = (id: string | undefined): string => {
    if (!id) return '—';
    return resp.device_names?.[id] ?? id;
  };

  const registered = resp.games
    .filter((g) => matches(g.name))
    .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
  const accelaOnly = extraAccela
    .filter((a) => matches(a.name))
    .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));

  if (registered.length === 0 && accelaOnly.length === 0) {
    const noResults = doc.createElement('div');
    noResults.textContent = `No matches for "${gamesSearchQuery}".`;
    noResults.style.cssText = 'color: #6b7280; padding: 16px; text-align: center;';
    tableBody.appendChild(noResults);
    return;
  }

  for (const game of registered) {
    tableBody.appendChild(renderGameRow(doc, game, resolveDeviceName));
  }
  for (const accela of accelaOnly) {
    tableBody.appendChild(renderAccelaOnlyRow(doc, accela));
  }
}

/** Header de la tabla: row de labels en uppercase. Columnas con
 *  anchos fijos para que las rows alineen visualmente. */
function buildTableHeader(doc: Document): HTMLElement {
  const header = doc.createElement('div');
  header.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 12px',
    'padding: 8px 14px',
    'border-bottom: 1px solid #2a2f42',
    'font-size: 11px',
    'color: #9aa3b2',
    'letter-spacing: 0.05em',
  ].join(';');

  const cell = (text: string, width: string) => {
    const c = doc.createElement('div');
    c.textContent = text;
    c.style.cssText = `width: ${width}; flex-shrink: 0;`;
    return c;
  };
  const flexCell = (text: string) => {
    const c = doc.createElement('div');
    c.textContent = text;
    c.style.cssText = 'flex: 1; min-width: 0;';
    return c;
  };

  // Hueco para el dot.
  header.appendChild(cell('', '10px'));
  header.appendChild(flexCell('NAME'));
  header.appendChild(cell('MODE', '70px'));
  header.appendChild(cell('AUTO SYNC', '90px'));
  header.appendChild(cell('LAST SYNCED FROM', '160px'));
  header.appendChild(cell('LAST SYNCED', '120px'));
  return header;
}

/** Row de un juego registrado en Ludusavi. Columnas: dot + name +
 *  mode + auto_sync + last_synced_from + last_sync_time. */
function renderGameRow(
  doc: Document,
  game: import('./daemon-client').ApiGameRow,
  resolveDeviceName: (id: string | undefined) => string,
): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 12px',
    'padding: 10px 14px',
    'border-bottom: 1px solid #1f2433',
    'font-size: 12px',
  ].join(';');

  // Dot status
  const dot = doc.createElement('div');
  dot.style.cssText = [
    `background: ${statusColor(game.status)}`,
    'width: 10px',
    'height: 10px',
    'border-radius: 50%',
    'flex-shrink: 0',
  ].join(';');
  row.appendChild(dot);

  // NAME (flex)
  const name = doc.createElement('div');
  name.textContent = game.name;
  name.title = describeGame(game); // tooltip con detalle si hay error/conflict
  name.style.cssText =
    'flex: 1; min-width: 0; font-size: 13px; color: #ffffff; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
  row.appendChild(name);

  // MODE
  row.appendChild(makeCell(doc, modeBadgeShort(game.mode), '70px', modeColor(game.mode)));

  // AUTO SYNC — checkmark/dash. Sólo aplicable si mode != none.
  const autoSyncText = game.mode === 'none' ? '—' : (game.auto_sync ? '✓' : '✗');
  const autoSyncColor = game.mode === 'none' ? '#6b7280' : (game.auto_sync ? '#3ecf8e' : '#9aa3b2');
  row.appendChild(makeCell(doc, autoSyncText, '90px', autoSyncColor));

  // LAST SYNCED FROM (device name).
  const fromText = game.last_synced_from
    ? resolveDeviceName(game.last_synced_from)
    : '—';
  row.appendChild(makeCell(doc, fromText, '160px', '#cfd6e3'));

  // LAST SYNCED (relative time).
  const lastSyncText = game.last_sync_time_utc
    ? relativeTime(game.last_sync_time_utc)
    : '—';
  row.appendChild(makeCell(doc, lastSyncText, '120px', '#cfd6e3'));

  return row;
}

/** Helper: celda con ancho fijo + color de texto. */
function makeCell(
  doc: Document,
  text: string,
  width: string,
  color: string,
): HTMLElement {
  const cell = doc.createElement('div');
  cell.textContent = text;
  cell.style.cssText = [
    `width: ${width}`,
    'flex-shrink: 0',
    `color: ${color}`,
    'overflow: hidden',
    'text-overflow: ellipsis',
    'white-space: nowrap',
  ].join(';');
  return cell;
}

/** Mode badge corto (uppercase) — paridad con la GUI. */
function modeBadgeShort(mode: string): string {
  switch (mode) {
    case 'sync':
      return 'SYNC';
    case 'cloud':
      return 'CLOUD';
    case 'local':
      return 'LOCAL';
    case 'none':
      return '—';
    default:
      return mode.toUpperCase() || '—';
  }
}

/** Color para el mode badge, distinto de status. */
function modeColor(mode: string): string {
  switch (mode) {
    case 'sync':
      return '#4f8ef7'; // azul
    case 'cloud':
      return '#3ecf8e'; // verde
    case 'local':
      return '#f0b400'; // amarillo
    default:
      return '#6b7280'; // gris para none
  }
}

/**
 * Row de un juego ACCELA-only (instalado en disco pero no registrado
 * todavía en Ludusavi). Mismo layout que renderGameRow, dashes en
 * todas las columnas de sync porque no hay info — la ausencia de
 * mode/auto_sync es la señal visual de "no configurado todavía".
 *
 * Sin badge "ACCELA" — los dashes en mode/auto_sync ya distinguen
 * claramente estos juegos de los registered.
 */
function renderAccelaOnlyRow(
  doc: Document,
  accela: { name: string; install_path: string; size_on_disk: number },
): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 12px',
    'padding: 10px 14px',
    'border-bottom: 1px solid #1f2433',
    'font-size: 12px',
  ].join(';');

  // Dot gris (no hay status real).
  const dot = doc.createElement('div');
  dot.style.cssText =
    'background: #6b7280; width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;';
  row.appendChild(dot);

  // NAME (flex). Tooltip con install_path para troubleshooting.
  const name = doc.createElement('div');
  name.textContent = accela.name;
  name.title = accela.install_path;
  name.style.cssText =
    'flex: 1; min-width: 0; font-size: 13px; color: #ffffff; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
  row.appendChild(name);

  // Resto de columnas: dashes.
  row.appendChild(makeCell(doc, '—', '70px', '#6b7280'));
  row.appendChild(makeCell(doc, '—', '90px', '#6b7280'));
  row.appendChild(makeCell(doc, '—', '160px', '#6b7280'));
  row.appendChild(makeCell(doc, '—', '120px', '#6b7280'));

  return row;
}


// ============================================================================
// Tab "This Device" — info del device actual + sus juegos
// ============================================================================

/**
 * Renderiza la tab "This Device" en paridad con `Screen::ThisDevice`
 * de la GUI Iced. Tres cards:
 *
 *  1. DEVICE — nombre, UUID completo, botón "Open logs".
 *  2. SYNC DAEMON — running dot, "Daemon is running", Last sync,
 *     Monitoring (lista inline de juegos en este device).
 *  3. CLOUD STORAGE — Provider, Remote ID, Path, rclone state +
 *     "Install instructions" si missing.
 *
 * Las 3 cards se cargan con datos de /api/devices, /api/status,
 * /api/cloud. Si alguno falla, mostramos un placeholder en su card
 * en lugar de abortar el render entero — el usuario aún ve los
 * datos que sí cargaron.
 */
async function loadAndRenderThisDevice(doc: Document, content: HTMLElement) {
  content.innerHTML = '';
  const loading = doc.createElement('div');
  loading.textContent = 'Cargando...';
  loading.style.cssText = 'color: #9aa3b2; font-size: 13px; padding: 12px;';
  content.appendChild(loading);

  // 4 fuentes en paralelo: devices (UUID, lista de juegos), status
  // (app_dir, last_sync), cloud (provider, rclone state), games
  // (cross para mostrar mode/status en monitoring inline).
  const [devicesSettled, statusSettled, cloudSettled] = await Promise.allSettled([
    daemon.getDevices(),
    daemon.getStatus(),
    daemon.getCloud(),
  ]);

  if (devicesSettled.status === 'rejected') {
    showFatalError(doc, content, devicesSettled.reason);
    return;
  }

  content.innerHTML = '';

  const devicesResp = devicesSettled.value;
  const thisDevice = devicesResp.devices.find((d) => d.is_current);

  if (!thisDevice) {
    const empty = doc.createElement('div');
    empty.textContent =
      'Este device no está registrado todavía. Ejecuta una operación de Ludusavi (backup, restore o sync) para inicializarlo.';
    empty.style.cssText = 'color: #9aa3b2; padding: 24px; text-align: center;';
    content.appendChild(empty);
    return;
  }

  // Card 1: DEVICE
  const appDir =
    statusSettled.status === 'fulfilled' ? statusSettled.value.app_dir : undefined;
  content.appendChild(renderDeviceCard(doc, thisDevice, appDir));

  // Card 2: SYNC DAEMON
  const lastSyncIso =
    statusSettled.status === 'fulfilled'
      ? statusSettled.value.last_sync_time_utc
      : undefined;
  content.appendChild(renderDaemonCard(doc, lastSyncIso, thisDevice.games));

  // Card 3: CLOUD STORAGE
  if (cloudSettled.status === 'fulfilled') {
    content.appendChild(renderCloudCard(doc, cloudSettled.value));
  } else {
    content.appendChild(
      renderCardError(doc, 'CLOUD STORAGE', `${cloudSettled.reason}`),
    );
  }
}

// ----------------------------------------------------------------------
// Cards de "This Device" — 3 secciones que mimica la GUI Iced.
// ----------------------------------------------------------------------

/** Card genérica con label de sección + cuerpo. Estilo común. */
function makeSectionCard(doc: Document, label: string): {
  card: HTMLElement;
  body: HTMLElement;
} {
  const card = doc.createElement('div');
  card.style.cssText = [
    'background: #171b26',
    'border: 1px solid #2a2f42',
    'border-radius: 6px',
    'padding: 16px',
    'margin-bottom: 12px',
  ].join(';');

  const labelEl = doc.createElement('div');
  labelEl.textContent = label;
  labelEl.style.cssText = [
    'font-size: 11px',
    'color: #9aa3b2',
    'letter-spacing: 0.08em',
    'margin-bottom: 10px',
    'font-weight: 600',
  ].join(';');
  card.appendChild(labelEl);

  const body = doc.createElement('div');
  body.style.cssText = 'display: flex; flex-direction: column; gap: 8px;';
  card.appendChild(body);

  return { card, body };
}

/** DEVICE card: nombre + UUID + Open logs. */
function renderDeviceCard(
  doc: Document,
  device: ApiDeviceRow,
  appDir: string | undefined,
): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'DEVICE');

  // Layout: nombre+UUID a la izquierda, botón a la derecha.
  const row = doc.createElement('div');
  row.style.cssText = 'display: flex; align-items: center; gap: 16px;';

  const left = doc.createElement('div');
  left.style.cssText = 'display: flex; flex-direction: column; gap: 4px; flex: 1;';

  const name = doc.createElement('div');
  name.textContent = device.name;
  name.style.cssText = 'font-size: 13px; color: #ffffff;';
  left.appendChild(name);

  const idEl = doc.createElement('div');
  idEl.textContent = device.id;
  idEl.style.cssText = 'font-size: 11px; color: #9aa3b2; font-family: monospace;';
  left.appendChild(idEl);

  row.appendChild(left);

  // Open logs button (sólo si app_dir disponible — si /api/status
  // falló no tenemos path al que llevar al usuario).
  if (appDir) {
    const btn = doc.createElement('button');
    btn.textContent = 'Open logs';
    btn.style.cssText = [
      'background: transparent',
      'color: #ffffff',
      'border: 1px solid #2a2f42',
      'border-radius: 4px',
      'padding: 6px 14px',
      'font-size: 12px',
      'font-family: inherit',
      'cursor: pointer',
    ].join(';');
    btn.addEventListener('click', async () => {
      const ok = await openAppDir(appDir);
      if (!ok) {
        btn.textContent = 'Open failed';
        btn.style.color = '#ef4444';
        setTimeout(() => {
          btn.textContent = 'Open logs';
          btn.style.color = '#ffffff';
        }, 2000);
      }
    });
    row.appendChild(btn);
  }

  body.appendChild(row);
  return card;
}

/** SYNC DAEMON card: dot running + "Daemon is running" + Last sync +
 *  Monitoring (lista inline de juegos del device). */
function renderDaemonCard(
  doc: Document,
  lastSyncIso: string | undefined,
  monitoredGames: string[],
): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'SYNC DAEMON');

  // Línea 1: dot verde + "Daemon is running".
  const runningRow = doc.createElement('div');
  runningRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const dot = doc.createElement('div');
  dot.style.cssText =
    'background: #3ecf8e; width: 8px; height: 8px; border-radius: 50%;';
  runningRow.appendChild(dot);
  const runningLabel = doc.createElement('div');
  runningLabel.textContent = 'Daemon is running';
  runningLabel.style.cssText = 'font-size: 13px; color: #3ecf8e;';
  runningRow.appendChild(runningLabel);
  body.appendChild(runningRow);

  // Línea 2: Last sync.
  const lastSyncRow = doc.createElement('div');
  lastSyncRow.style.cssText = 'display: flex; gap: 6px; font-size: 12px;';
  const lastSyncLabel = doc.createElement('span');
  lastSyncLabel.textContent = 'Last sync:';
  lastSyncLabel.style.color = '#9aa3b2';
  lastSyncRow.appendChild(lastSyncLabel);
  const lastSyncValue = doc.createElement('span');
  lastSyncValue.textContent = relativeTime(lastSyncIso);
  lastSyncValue.style.color = '#cfd6e3';
  lastSyncRow.appendChild(lastSyncValue);
  body.appendChild(lastSyncRow);

  // Línea 3+: Monitoring.
  const monitoringWrap = doc.createElement('div');
  monitoringWrap.style.cssText = 'display: flex; flex-direction: column; gap: 4px;';
  const monitoringLabel = doc.createElement('div');
  monitoringLabel.textContent = 'Monitoring:';
  monitoringLabel.style.cssText = 'font-size: 12px; color: #9aa3b2;';
  monitoringWrap.appendChild(monitoringLabel);
  const monitoringValue = doc.createElement('div');
  if (monitoredGames.length === 0) {
    monitoringValue.textContent = 'No games registered for this device';
    monitoringValue.style.color = '#9aa3b2';
  } else {
    const sorted = [...monitoredGames].sort((a, b) =>
      a.toLowerCase().localeCompare(b.toLowerCase()),
    );
    monitoringValue.textContent = sorted.join(', ');
    monitoringValue.style.color = '#cfd6e3';
  }
  monitoringValue.style.cssText += ';font-size: 12px; line-height: 1.5;';
  monitoringWrap.appendChild(monitoringValue);
  body.appendChild(monitoringWrap);

  return card;
}

/** CLOUD STORAGE card: provider/remote_id/path/rclone state. */
function renderCloudCard(doc: Document, cloud: ApiCloudResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'CLOUD STORAGE');

  const addRow = (label: string, value: string, valueColor = '#cfd6e3') => {
    const row = doc.createElement('div');
    row.style.cssText = 'display: flex; gap: 6px; font-size: 12px;';
    const lab = doc.createElement('span');
    lab.textContent = `${label}:`;
    lab.style.color = '#9aa3b2';
    row.appendChild(lab);
    const val = doc.createElement('span');
    val.textContent = value;
    val.style.color = valueColor;
    row.appendChild(val);
    body.appendChild(row);
  };

  addRow('Provider', cloud.provider, '#ffffff');
  addRow('Remote', cloud.remote_id);
  addRow('Path', cloud.path);

  // Línea de rclone state: dot de color + texto.
  const rcloneRow = doc.createElement('div');
  rcloneRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const rcDot = doc.createElement('div');
  let rcColor = '#9aa3b2';
  let rcText = 'rclone not configured';
  let rcTextColor = '#9aa3b2';
  if (cloud.rclone_state === 'missing') {
    rcColor = '#ef4444';
    rcText = 'rclone not found';
    rcTextColor = '#9aa3b2';
  } else if (cloud.rclone_state === 'ok') {
    rcColor = '#3ecf8e';
    rcText = 'rclone configured';
    rcTextColor = '#3ecf8e';
  }
  rcDot.style.cssText = `background: ${rcColor}; width: 8px; height: 8px; border-radius: 50%;`;
  rcloneRow.appendChild(rcDot);
  const rcLabel = doc.createElement('span');
  rcLabel.textContent = rcText;
  rcLabel.style.cssText = `font-size: 13px; color: ${rcTextColor};`;
  rcloneRow.appendChild(rcLabel);
  // Botón Install instructions cuando missing.
  if (cloud.rclone_state === 'missing' && cloud.install_url) {
    const install = doc.createElement('button');
    install.textContent = 'Install instructions';
    install.style.cssText = [
      'background: #4f8ef7',
      'color: white',
      'border: none',
      'border-radius: 4px',
      'padding: 4px 12px',
      'font-size: 11px',
      'font-family: inherit',
      'cursor: pointer',
      'margin-left: 6px',
    ].join(';');
    install.addEventListener('click', () => {
      // Plugin no puede abrir URLs externas directamente desde el
      // overlay (sandbox). Usamos `SteamClient.URL.ExecuteSteamURL`
      // si está disponible, o `window.open` (que en CEF puede abrir
      // en el navegador externo según política).
      try {
        (doc.defaultView as any)?.open(cloud.install_url, '_blank');
      } catch (e) {
        console.error('[ludusavi-sync] could not open install_url:', e);
      }
    });
    rcloneRow.appendChild(install);
  }
  body.appendChild(rcloneRow);

  return card;
}

/** Card de error: cuando un fetch para una sub-card falla, en lugar
 *  de abortar el render entero pintamos el card en estado de error. */
function renderCardError(doc: Document, label: string, msg: string): HTMLElement {
  const { card, body } = makeSectionCard(doc, label);
  const err = doc.createElement('div');
  err.textContent = `✗ ${msg}`;
  err.style.cssText = 'color: #ef4444; font-size: 12px;';
  body.appendChild(err);
  return card;
}

// ============================================================================
// Tab "All Devices" — lista de todos los devices conocidos
// ============================================================================

/**
 * Renderiza la tab "All Devices" en paridad con `Screen::AllDevices`
 * de la GUI Iced.
 *
 * Lógica clave: la GUI sólo muestra devices que tienen ≥1 juego en
 * SYNC mode. CLOUD/LOCAL/NONE no participan en multi-device sync, así
 * que sus devices no son relevantes en esta vista. Replicamos ese
 * filtro en el cliente — la API /api/devices no filtra por mode (la
 * usan también otros consumidores que necesitan la lista completa).
 *
 * Cada card muestra:
 *  - Nombre/UUID del device + tag "THIS DEVICE" si aplica.
 *  - "{N} game(s): name1, name2..." (lista inline separada por coma).
 *  - "Last sync: 5 hours ago".
 *
 * Header del overlay muestra "{N} device(s) registered" donde N es
 * el count post-filtro (sólo devices con SYNC games).
 */
async function loadAndRenderAllDevices(doc: Document, content: HTMLElement) {
  content.innerHTML = '';
  const loading = doc.createElement('div');
  loading.textContent = 'Cargando...';
  loading.style.cssText = 'color: #9aa3b2; font-size: 13px; padding: 12px;';
  content.appendChild(loading);

  // Necesitamos /api/devices (lista de devices con sus juegos) +
  // /api/games (mode por juego, para filtrar a SYNC-only).
  const [devicesSettled, gamesSettled] = await Promise.allSettled([
    daemon.getDevices(),
    daemon.getGames(),
  ]);

  if (devicesSettled.status === 'rejected') {
    showFatalError(doc, content, devicesSettled.reason);
    return;
  }
  if (gamesSettled.status === 'rejected') {
    showFatalError(doc, content, gamesSettled.reason);
    return;
  }

  content.innerHTML = '';

  const devicesResp = devicesSettled.value;
  const gamesResp = gamesSettled.value;

  // Set de juegos en SYNC mode. Mismo criterio que la GUI Iced.
  const syncGameNames = new Set(
    gamesResp.games.filter((g) => g.mode === 'sync').map((g) => g.name),
  );

  // Para cada device, intersectar device.games con syncGameNames.
  // Conservamos sólo los devices con ≥1 juego en SYNC. Construimos
  // ApiDeviceRow-like objects con games filtrados.
  type FilteredDevice = ApiDeviceRow & { syncGames: string[] };
  const filteredDevices: FilteredDevice[] = devicesResp.devices
    .map((d) => ({
      ...d,
      syncGames: d.games.filter((g) => syncGameNames.has(g)),
    }))
    .filter((d) => d.syncGames.length > 0);

  // Header: "{N} device(s) registered".
  const summary = doc.createElement('div');
  const count = filteredDevices.length;
  summary.textContent = `${count} device${count === 1 ? '' : 's'} registered`;
  summary.style.cssText = 'color: #9aa3b2; font-size: 12px; margin-bottom: 12px;';
  content.appendChild(summary);

  if (filteredDevices.length === 0) {
    const empty = doc.createElement('div');
    empty.textContent =
      'No devices found. Run a sync to register devices.';
    empty.style.cssText = 'color: #9aa3b2; padding: 24px; text-align: center;';
    content.appendChild(empty);
    return;
  }

  // Sort: current device primero, luego alfabético por name.
  filteredDevices.sort((a, b) => {
    if (a.is_current && !b.is_current) return -1;
    if (!a.is_current && b.is_current) return 1;
    return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
  });

  for (const device of filteredDevices) {
    content.appendChild(renderAllDevicesCard(doc, device, device.syncGames));
  }
}

/**
 * Card de un device en la lista All Devices. Mismo layout que la GUI:
 *  - Nombre + tag "THIS DEVICE" si current.
 *  - "{N} game(s): name1, name2..." inline.
 *  - "Last sync: relativeTime"
 */
function renderAllDevicesCard(
  doc: Document,
  device: ApiDeviceRow,
  syncGames: string[],
): HTMLElement {
  const card = doc.createElement('div');
  card.style.cssText = [
    'background: #171b26',
    device.is_current ? 'border: 1px solid #4f8ef7' : 'border: 1px solid #2a2f42',
    'border-radius: 6px',
    'padding: 16px',
    'margin-bottom: 12px',
    'display: flex',
    'flex-direction: column',
    'gap: 8px',
  ].join(';');

  // Línea 1: nombre + "THIS DEVICE" tag.
  const nameRow = doc.createElement('div');
  nameRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const name = doc.createElement('div');
  name.textContent = device.name;
  name.style.cssText = 'font-size: 13px; color: #cfd6e3; flex: 1; min-width: 0;';
  nameRow.appendChild(name);
  if (device.is_current) {
    const tag = doc.createElement('div');
    tag.textContent = 'THIS DEVICE';
    tag.style.cssText = [
      'font-size: 11px',
      'color: #4f8ef7',
      'letter-spacing: 0.05em',
    ].join(';');
    nameRow.appendChild(tag);
  }
  card.appendChild(nameRow);

  // Línea 2: "{N} game(s): name1, name2..."
  const gamesLine = doc.createElement('div');
  const sortedGames = [...syncGames].sort((a, b) =>
    a.toLowerCase().localeCompare(b.toLowerCase()),
  );
  const plural = sortedGames.length === 1 ? '' : 's';
  gamesLine.textContent = `${sortedGames.length} game${plural}: ${sortedGames.join(', ')}`;
  gamesLine.style.cssText = 'font-size: 12px; color: #9aa3b2; line-height: 1.5;';
  card.appendChild(gamesLine);

  // Línea 3: Last sync.
  const lastSyncRow = doc.createElement('div');
  lastSyncRow.style.cssText = 'display: flex; gap: 6px; font-size: 11px;';
  const lastSyncLabel = doc.createElement('span');
  lastSyncLabel.textContent = 'Last sync:';
  lastSyncLabel.style.color = '#9aa3b2';
  lastSyncRow.appendChild(lastSyncLabel);
  const lastSyncValue = doc.createElement('span');
  lastSyncValue.textContent = relativeTime(device.last_sync_time_utc);
  lastSyncValue.style.color = '#cfd6e3';
  lastSyncRow.appendChild(lastSyncValue);
  card.appendChild(lastSyncRow);

  return card;
}


/** Renderiza un mensaje de error fatal en `content` y limpia el resto. */
function showFatalError(doc: Document, content: HTMLElement, e: unknown) {
  content.innerHTML = '';
  const err = doc.createElement('div');
  err.textContent = `✗ ${e}`;
  err.style.cssText = 'color: #ef4444; font-size: 13px; padding: 12px;';
  content.appendChild(err);
}

// ============================================================================
// Tab "Settings" — 6 cards en paridad con Screen::Other de la GUI
// ============================================================================

/**
 * Renderiza la tab "Settings" replicando las 6 cards de Screen::Other
 * en la GUI Iced. Read-only en Fase 1 — los toggles, pickers y botones
 * (Install service, Refresh manifest, Get rclone, etc.) se enchufan
 * con POST endpoints en Fase 2.
 *
 * Cards (mismo orden que la GUI):
 *  1. LOCAL          — backup path
 *  2. CLOUD/RCLONE   — provider + remote + path + rclone executable +
 *                      rclone arguments + estado del binario
 *  3. DAEMON         — service installed + daemon running
 *  4. SAFETY         — safety_backups + system_notifications toggles
 *  5. ROOTS          — lista de (store, path)
 *  6. MANIFEST       — primary URL + lista de secondary
 *
 * Pedimos /api/settings + /api/cloud en paralelo (cloud para la card
 * CLOUD/RCLONE, settings para todo lo demás).
 */
async function loadAndRenderSettings(doc: Document, content: HTMLElement) {
  content.innerHTML = '';
  const loading = doc.createElement('div');
  loading.textContent = 'Cargando...';
  loading.style.cssText = 'color: #9aa3b2; font-size: 13px; padding: 12px;';
  content.appendChild(loading);

  const [settingsSettled, cloudSettled] = await Promise.allSettled([
    daemon.getSettings(),
    daemon.getCloud(),
  ]);

  if (settingsSettled.status === 'rejected') {
    showFatalError(doc, content, settingsSettled.reason);
    return;
  }

  content.innerHTML = '';
  const settings = settingsSettled.value;

  // 1. LOCAL
  content.appendChild(renderLocalCard(doc, settings));

  // 2. CLOUD/RCLONE — necesita /api/cloud. Si falló mostramos
  //    placeholder en lugar de abortar render entero.
  if (cloudSettled.status === 'fulfilled') {
    content.appendChild(renderCloudRcloneCard(doc, cloudSettled.value));
  } else {
    content.appendChild(
      renderCardError(doc, 'CLOUD / RCLONE', `${cloudSettled.reason}`),
    );
  }

  // 3. DAEMON
  content.appendChild(renderDaemonServiceCard(doc, settings));

  // 4. SAFETY
  content.appendChild(renderSafetyCard(doc, settings));

  // 5. ROOTS
  content.appendChild(renderRootsCard(doc, settings));

  // 6. MANIFEST
  content.appendChild(renderManifestCard(doc, settings));
}

/** Helper de Settings: row label:value alineado horizontal. */
function settingsRow(
  doc: Document,
  label: string,
  value: string,
  opts?: { mono?: boolean; valueColor?: string },
): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = 'display: flex; gap: 12px; font-size: 12px; align-items: baseline;';
  const labelEl = doc.createElement('span');
  labelEl.textContent = `${label}:`;
  labelEl.style.cssText = 'color: #9aa3b2; min-width: 140px; flex-shrink: 0;';
  row.appendChild(labelEl);
  const valEl = doc.createElement('span');
  valEl.textContent = value || '—';
  valEl.style.cssText = [
    `color: ${opts?.valueColor ?? '#cfd6e3'}`,
    opts?.mono ? 'font-family: monospace; font-size: 11px;' : '',
    'overflow: hidden',
    'text-overflow: ellipsis',
    'word-break: break-all',
  ].join(';');
  row.appendChild(valEl);
  return row;
}

/** Helper de Settings: línea de descripción gris bajo el label. */
function settingsDescription(doc: Document, text: string): HTMLElement {
  const el = doc.createElement('div');
  el.textContent = text;
  el.style.cssText = 'color: #9aa3b2; font-size: 12px; margin-bottom: 4px;';
  return el;
}

/** Helper de Settings: pill ON/OFF + descripción. Si se pasa
 *  `onToggle`, el pill es clickable y llama el callback con el valor
 *  invertido — la actualización del estado real (POST + refresh) la
 *  hace el caller. Sin `onToggle` queda como display-only. */
function settingsToggleRow(
  doc: Document,
  enabled: boolean,
  description: string,
  onToggle?: (newValue: boolean) => Promise<void>,
): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = 'display: flex; gap: 12px; align-items: center;';
  const pill = doc.createElement('div');
  pill.textContent = enabled ? 'ON' : 'OFF';
  pill.style.cssText = [
    'padding: 4px 12px',
    'border-radius: 4px',
    'font-size: 11px',
    'font-weight: 600',
    'letter-spacing: 0.05em',
    enabled ? 'background: #4f8ef7' : 'background: #2a2f42',
    enabled ? 'color: white' : 'color: #9aa3b2',
    'flex-shrink: 0',
    onToggle ? 'cursor: pointer; user-select: none' : '',
  ].join(';');
  if (onToggle) {
    pill.addEventListener('click', async () => {
      // Optimistic UI: marcamos como "...pending" para que el usuario
      // sepa que su click se registró. SSE refrescará la card con el
      // valor real cuando el daemon escriba sync-games.json.
      const original = pill.textContent;
      pill.textContent = '…';
      pill.style.opacity = '0.5';
      try {
        await onToggle(!enabled);
        // No restauramos pill aquí — el SSE event `daemon_restarted`
        // disparará un re-render completo de la tab Settings que ya
        // muestra el nuevo valor. Si por alguna razón eso no llega,
        // restauramos como fallback tras 2s.
        setTimeout(() => {
          if (pill.textContent === '…') {
            pill.textContent = original;
            pill.style.opacity = '1';
          }
        }, 2000);
      } catch (e) {
        console.error('[ludusavi-sync] toggle failed:', e);
        pill.textContent = original;
        pill.style.opacity = '1';
      }
    });
  }
  row.appendChild(pill);
  const desc = doc.createElement('span');
  desc.textContent = description;
  desc.style.cssText = 'font-size: 12px; color: #cfd6e3;';
  row.appendChild(desc);
  return row;
}

// --- Cards individuales -----------------------------------------------------

function renderLocalCard(doc: Document, settings: ApiSettingsResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'LOCAL');
  body.appendChild(
    settingsDescription(doc, 'Local ZIP backups are stored in this directory.'),
  );
  body.appendChild(settingsRow(doc, 'Backup path', settings.backup_path, { mono: true }));
  return card;
}

function renderCloudRcloneCard(doc: Document, cloud: ApiCloudResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'CLOUD / RCLONE');
  body.appendChild(
    settingsDescription(doc, 'Configure rclone and your cloud provider for save syncing.'),
  );

  // rclone executable + arguments.
  body.appendChild(
    settingsRow(doc, 'rclone executable', cloud.rclone_executable ?? '—', { mono: true }),
  );
  body.appendChild(
    settingsRow(doc, 'rclone arguments', cloud.rclone_arguments ?? '—', { mono: true }),
  );

  // Provider / Remote / Path.
  body.appendChild(settingsRow(doc, 'Provider', cloud.provider, { valueColor: '#ffffff' }));
  body.appendChild(settingsRow(doc, 'Remote', cloud.remote_id));
  body.appendChild(settingsRow(doc, 'Cloud path', cloud.path));

  // rclone state — dot + texto coloreados según estado.
  const stateRow = doc.createElement('div');
  stateRow.style.cssText = 'display: flex; align-items: center; gap: 8px; margin-top: 4px;';
  const dot = doc.createElement('div');
  let dotColor = '#9aa3b2';
  let stateText = 'rclone not configured';
  let textColor = '#9aa3b2';
  if (cloud.rclone_state === 'missing') {
    dotColor = '#ef4444';
    stateText = 'rclone not found';
  } else if (cloud.rclone_state === 'ok') {
    dotColor = '#3ecf8e';
    stateText = 'rclone configured';
    textColor = '#3ecf8e';
  }
  dot.style.cssText = `background: ${dotColor}; width: 8px; height: 8px; border-radius: 50%;`;
  stateRow.appendChild(dot);
  const txt = doc.createElement('span');
  txt.textContent = stateText;
  txt.style.cssText = `font-size: 13px; color: ${textColor};`;
  stateRow.appendChild(txt);
  body.appendChild(stateRow);

  return card;
}

function renderDaemonServiceCard(
  doc: Document,
  settings: ApiSettingsResponse,
): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'DAEMON');
  body.appendChild(
    settingsDescription(doc, 'Install or uninstall the sync daemon as a system service.'),
  );

  // Service installed dot + texto.
  const installedRow = doc.createElement('div');
  installedRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const installDot = doc.createElement('div');
  installDot.style.cssText = `background: ${
    settings.service.installed ? '#3ecf8e' : '#9aa3b2'
  }; width: 8px; height: 8px; border-radius: 50%;`;
  installedRow.appendChild(installDot);
  const installLabel = doc.createElement('span');
  installLabel.textContent = settings.service.installed
    ? 'Service installed'
    : 'Service not installed';
  installLabel.style.cssText = `font-size: 13px; color: ${
    settings.service.installed ? '#3ecf8e' : '#9aa3b2'
  };`;
  installedRow.appendChild(installLabel);
  body.appendChild(installedRow);

  // Daemon running — siempre true en este punto (si el plugin no
  // pudiera conectarse no estaríamos en esta render).
  const runningRow = doc.createElement('div');
  runningRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const runDot = doc.createElement('div');
  runDot.style.cssText = 'background: #3ecf8e; width: 8px; height: 8px; border-radius: 50%;';
  runningRow.appendChild(runDot);
  const runLabel = doc.createElement('span');
  runLabel.textContent = 'Daemon is running';
  runLabel.style.cssText = 'font-size: 13px; color: #3ecf8e;';
  runningRow.appendChild(runLabel);
  body.appendChild(runningRow);

  return card;
}

function renderSafetyCard(doc: Document, settings: ApiSettingsResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'SAFETY');
  body.appendChild(
    settingsDescription(
      doc,
      'Before overwriting your saves on download or restore, keep a local copy you can revert to. Applies to games under 500 MB.',
    ),
  );
  body.appendChild(
    settingsToggleRow(
      doc,
      settings.safety.safety_backups_enabled,
      'Safety backups before destructive operations',
      async (newValue) => {
        await daemon.setSafety({ safety_backups_enabled: newValue });
      },
    ),
  );
  body.appendChild(
    settingsToggleRow(
      doc,
      settings.safety.system_notifications_enabled,
      'System notifications when daemon syncs in background',
      async (newValue) => {
        await daemon.setSafety({ system_notifications_enabled: newValue });
      },
    ),
  );
  return card;
}

function renderRootsCard(doc: Document, settings: ApiSettingsResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'ROOTS');
  body.appendChild(
    settingsDescription(
      doc,
      'Game roots are required to detect save file locations automatically.',
    ),
  );
  if (settings.roots.length === 0) {
    const empty = doc.createElement('div');
    empty.textContent = 'No roots configured.';
    empty.style.cssText = 'color: #6b7280; font-size: 12px;';
    body.appendChild(empty);
    return card;
  }
  for (const root of settings.roots) {
    const row = doc.createElement('div');
    row.style.cssText = 'display: flex; gap: 12px; font-size: 12px; align-items: baseline;';
    const storeBadge = doc.createElement('span');
    storeBadge.textContent = formatStoreLabel(root.store);
    storeBadge.style.cssText = [
      'color: #4f8ef7',
      'min-width: 140px',
      'flex-shrink: 0',
      'font-weight: 500',
    ].join(';');
    row.appendChild(storeBadge);
    const path = doc.createElement('span');
    path.textContent = root.path;
    path.style.cssText =
      'color: #cfd6e3; font-family: monospace; font-size: 11px; word-break: break-all;';
    row.appendChild(path);
    body.appendChild(row);
  }
  return card;
}

/** Convierte el camelCase del API a un label más legible para el usuario. */
function formatStoreLabel(store: string): string {
  switch (store) {
    case 'steam':
      return 'Steam';
    case 'epic':
      return 'Epic';
    case 'gog':
      return 'GOG';
    case 'gogGalaxy':
      return 'GOG Galaxy';
    case 'heroic':
      return 'Heroic';
    case 'legendary':
      return 'Legendary';
    case 'lutris':
      return 'Lutris';
    case 'microsoft':
      return 'Microsoft Store';
    case 'origin':
      return 'Origin';
    case 'prime':
      return 'Prime Gaming';
    case 'uplay':
      return 'Uplay';
    case 'ea':
      return 'EA';
    case 'otherHome':
      return 'Other (home)';
    case 'otherWine':
      return 'Other (Wine)';
    case 'otherWindows':
      return 'Other (Windows)';
    case 'otherLinux':
      return 'Other (Linux)';
    case 'otherMac':
      return 'Other (Mac)';
    case 'other':
      return 'Other';
    default:
      return store;
  }
}

function renderManifestCard(doc: Document, settings: ApiSettingsResponse): HTMLElement {
  const { card, body } = makeSectionCard(doc, 'MANIFEST');
  body.appendChild(
    settingsDescription(
      doc,
      'The manifest contains the list of known games and their save locations.',
    ),
  );

  // Primary manifest.
  const primaryRow = doc.createElement('div');
  primaryRow.style.cssText = 'display: flex; gap: 12px; font-size: 12px; align-items: baseline;';
  const primaryLabel = doc.createElement('span');
  primaryLabel.textContent = settings.manifest.primary_enabled
    ? 'Primary (enabled):'
    : 'Primary (disabled):';
  primaryLabel.style.cssText = `color: ${
    settings.manifest.primary_enabled ? '#3ecf8e' : '#9aa3b2'
  }; min-width: 140px; flex-shrink: 0;`;
  primaryRow.appendChild(primaryLabel);
  const primaryUrl = doc.createElement('span');
  primaryUrl.textContent = settings.manifest.primary_url;
  primaryUrl.style.cssText =
    'color: #cfd6e3; font-family: monospace; font-size: 11px; word-break: break-all;';
  primaryRow.appendChild(primaryUrl);
  body.appendChild(primaryRow);

  // Secondary manifests.
  if (settings.manifest.secondary.length > 0) {
    const sep = doc.createElement('div');
    sep.style.cssText = 'border-top: 1px solid #2a2f42; margin: 8px 0 4px 0;';
    body.appendChild(sep);
    for (const sec of settings.manifest.secondary) {
      const row = doc.createElement('div');
      row.style.cssText =
        'display: flex; gap: 12px; font-size: 12px; align-items: baseline;';
      const label = doc.createElement('span');
      const kindLabel = sec.kind === 'remote' ? 'Secondary (remote)' : 'Secondary (local)';
      label.textContent = `${kindLabel}${sec.enabled ? '' : ' [disabled]'}:`;
      label.style.cssText = `color: ${
        sec.enabled ? '#cfd6e3' : '#6b7280'
      }; min-width: 140px; flex-shrink: 0;`;
      row.appendChild(label);
      const value = doc.createElement('span');
      value.textContent = sec.kind === 'remote' ? sec.url : sec.path;
      value.style.cssText =
        'color: #cfd6e3; font-family: monospace; font-size: 11px; word-break: break-all;';
      row.appendChild(value);
      body.appendChild(row);
    }
  }

  return card;
}

// ============================================================================
// SSE wiring — refresh automático
// ============================================================================

/**
 * Abre la conexión SSE al daemon y registra handlers para refrescar
 * el overlay cuando llegan eventos. Cierra cualquier conexión previa
 * antes de abrir la nueva (idempotente).
 *
 * El daemon emite tres tipos de eventos (ver `DaemonEvent` en
 * daemon-client.ts):
 *  - `games_changed`     → la lista de juegos cambió (añadir, mode,
 *                           status). Refrescamos.
 *  - `devices_changed`   → cambió la lista de devices (afecta nombres
 *                           que aparecen en cards). Refrescamos.
 *  - `daemon_restarted`  → el daemon se relanzó (token podría haber
 *                           rotado). Refrescamos full.
 *
 * Todos disparan el mismo `scheduleRefresh()`. Si llegan varios en
 * ráfaga (file watcher es ruidoso) sólo refetcheamos una vez gracias
 * al debounce.
 */
export async function connectSse(
  doc: Document,
  overlay: HTMLElement,
  content: HTMLElement,
  statusPill: HTMLElement,
) {
  disconnectSse();
  applyStatusPill(statusPill, 'connecting');

  let es: EventSource;
  try {
    es = await daemon.subscribeEvents(
      (event) => {
        if (!shouldRefreshOn(event.type)) return;
        scheduleRefresh(doc, overlay, content);
        // Llegó un evento = la conexión funciona.
        applyStatusPill(statusPill, 'live');
      },
      // EventSource.onerror se dispara tanto al fallar la conexión
      // inicial como en reconexiones intermedias. El navegador hace
      // backoff automático y vuelve a abrir, así que no destruimos
      // el EventSource — sólo marcamos status.
      () => {
        applyStatusPill(statusPill, 'reconnecting');
      },
    );
  } catch (e) {
    console.error('[ludusavi-sync] SSE subscribe failed:', e);
    applyStatusPill(statusPill, 'offline');
    return;
  }

  // `open` se dispara cuando la conexión queda establecida — usamos
  // ese momento para marcar 'live' aunque aún no haya llegado ningún
  // evento real.
  es.addEventListener('open', () => applyStatusPill(statusPill, 'live'));

  currentEventSource = es;
}

/**
 * Decide si un evento SSE es relevante para la tab activa actual.
 * Optimización: si estoy en Games y llega `devices_changed`, no
 * refetcheo Games. Si estoy en This Device / All Devices, ambos
 * eventos son relevantes (games_changed afecta los nombres / status
 * que se muestran).
 *
 * `daemon_restarted` siempre fuerza refresh — el token podría haber
 * rotado y mejor recargar.
 */
function shouldRefreshOn(eventType: string): boolean {
  if (eventType === 'daemon_restarted') return true;
  switch (currentTab) {
    case 'games':
      return eventType === 'games_changed';
    case 'this-device':
    case 'all-devices':
      return eventType === 'devices_changed' || eventType === 'games_changed';
    case 'settings':
      // Settings refleja config + sync_games_config. El daemon no
      // emite un evento `settings_changed` específico todavía — pero
      // como SyncGamesConfig (toggles SAFETY) se escribe al cambiar
      // mode de un juego, `daemon_restarted` ya cubre el caso (se
      // emite cuando se rota sync-games.json). Refrescamos también
      // ahí para que toggles edits desde la GUI se vean en vivo.
      return false;
  }
}

/** Cierra el EventSource activo y cancela cualquier refresh pendiente. */
export function disconnectSse() {
  if (currentEventSource) {
    currentEventSource.close();
    currentEventSource = null;
  }
  if (pendingRefreshTimer !== null) {
    clearTimeout(pendingRefreshTimer);
    pendingRefreshTimer = null;
  }
}

/**
 * Debounce: si llegan varios eventos en ventana de 200ms, refetchear
 * una sola vez. El file watcher del daemon emite eventos por cada
 * fichero modificado — escribir game-list + status puede disparar 2
 * eventos seguidos.
 *
 * Refresca la tab activa, no necesariamente Games.
 */
function scheduleRefresh(doc: Document, overlay: HTMLElement, content: HTMLElement) {
  if (pendingRefreshTimer !== null) {
    clearTimeout(pendingRefreshTimer);
  }
  pendingRefreshTimer = setTimeout(() => {
    pendingRefreshTimer = null;
    renderActiveTab(doc, overlay, content);
  }, 200);
}

/** Renderiza el status pill: dot + texto según estado SSE. */
function applyStatusPill(pill: HTMLElement, status: SseStatus) {
  pill.setAttribute('data-sse-status', status);
  // Limpia y reconstruye con dot + texto.
  pill.innerHTML = '';
  const dot = pill.ownerDocument.createElement('span');
  dot.style.cssText = [
    'width: 7px',
    'height: 7px',
    'border-radius: 50%',
    'flex-shrink: 0',
  ].join(';');
  const label = pill.ownerDocument.createElement('span');
  switch (status) {
    case 'live':
      dot.style.background = '#3ecf8e';
      label.textContent = 'Live';
      break;
    case 'connecting':
      dot.style.background = '#eab308';
      label.textContent = 'Connecting…';
      break;
    case 'reconnecting':
      dot.style.background = '#f97316';
      label.textContent = 'Reconnecting…';
      break;
    case 'offline':
      dot.style.background = '#ef4444';
      label.textContent = 'Offline (manual refresh)';
      break;
  }
  pill.appendChild(dot);
  pill.appendChild(label);
}
