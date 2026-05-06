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
import { ApiCloudResponse, ApiDeviceRow, daemon, openAppDir } from './daemon-client';
import { describeGame, statusColor } from './game-format';
import { relativeTime } from './time-format';

declare const g_PopupManager: any;

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
  if (typeof g_PopupManager === 'undefined') {
    return '[ERROR] g_PopupManager no definido';
  }
  const popup = g_PopupManager.GetExistingPopup(MAIN_WINDOW_NAME);
  if (!popup) return '[ERROR] Main window no disponible';
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
  syncTab.style.outline = '2px solid #4f8ef7';
  syncTab.style.borderRadius = '4px';

  syncTab.addEventListener(
    'click',
    (e) => {
      e.stopPropagation();
      e.preventDefault();
      showSyncOverlay(doc);
      syncTab.style.outline = '2px solid #3ecf8e';
    },
    true, // capture phase para llegar antes de los listeners de Steam
  );

  // Listeners en los OTROS tabs de la nav: cuando el usuario clica
  // Biblioteca/Tienda/Comunidad/etc, ocultamos nuestro overlay y
  // restauramos el outline azul de SYNC. Steam sigue procesando el
  // click normalmente porque NO usamos stopPropagation aquí.
  Array.from(row.children).forEach((sibling) => {
    if (sibling === syncTab) return;
    sibling.addEventListener(
      'click',
      () => {
        hideSyncOverlay(doc);
        syncTab.style.outline = '2px solid #4f8ef7';
      },
      // No capture: Steam tiene que procesar el click primero.
    );
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
 *  (Screen::Games, Screen::ThisDevice, Screen::AllDevices). */
type ActiveTab = 'games' | 'this-device' | 'all-devices';

/** Tab activa entre llamadas a showSyncOverlay. Se preserva si el
 *  usuario abre/cierra el overlay sin reiniciar Steam. */
let currentTab: ActiveTab = 'games';

const TABS: { id: ActiveTab; label: string }[] = [
  { id: 'games', label: 'Games' },
  { id: 'this-device', label: 'This Device' },
  { id: 'all-devices', label: 'All Devices' },
];

/**
 * Muestra el overlay del SYNC tab dentro del main window de Steam.
 * Idempotente: si ya está visible recarga la tab activa. Conecta al
 * SSE stream para refrescar automáticamente en cambios.
 *
 * Se renderiza con vanilla DOM (sin React) porque estamos fuera del
 * tree del settings panel. Para eventos interactivos basta con
 * addEventListener directo.
 */
async function showSyncOverlay(doc: Document) {
  let overlay = doc.querySelector<HTMLElement>(`[${OVERLAY_ATTR}]`);

  if (!overlay) {
    overlay = buildOverlayShell(doc);
    doc.body.appendChild(overlay);
  } else {
    overlay.style.display = 'flex';
  }

  const content = overlay.querySelector<HTMLElement>('[data-content]')!;
  const statusPill = overlay.querySelector<HTMLElement>('[data-sse-status]')!;
  await renderActiveTab(doc, overlay, content);
  // Conectar SSE después del primer render — si la conexión falla no
  // bloquea ver la lista inicial. El refresh automático sólo aplica a
  // futuras actualizaciones.
  connectSse(doc, overlay, content, statusPill);
}

/** Renderiza la tab activa actual en el content area. */
async function renderActiveTab(
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
function buildOverlayShell(doc: Document): HTMLElement {
  const overlay = doc.createElement('div');
  overlay.setAttribute(OVERLAY_ATTR, '1');
  overlay.style.cssText = [
    'position: fixed',
    'top: 60px',
    'left: 0',
    'right: 0',
    'bottom: 0',
    'z-index: 9000',
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
 * Fetch a /api/games + /api/accela-installs en paralelo y render como
 * lista combinada de cards en `content`.
 *
 * /api/games viene de las 3 fuentes que ya gestiona el worker (manifest
 * + custom + backups previos). /api/accela-installs es la 4ª fuente que
 * el GUI Iced también añade al game-list. Mergeamos por nombre: si un
 * juego está en ambos, gana la entry de /api/games (tiene info de
 * sync). Si sólo está en accela-installs, lo mostramos como "📦
 * Installed (ACCELA)" sin info de sync.
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
  // Usamos `Promise.allSettled` para que un fallo del segundo no cancele
  // el primero.
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

  // Header summary line.
  const summary = doc.createElement('div');
  const rcloneNote = resp.rclone_missing ? ' · ⚠ rclone no disponible' : '';
  const totalCount = resp.games.length + extraAccela.length;
  const accelaCount =
    extraAccela.length > 0 ? ` (+${extraAccela.length} ACCELA)` : '';
  summary.textContent = `${totalCount} juego(s)${accelaCount} — device: ${resp.device.name}${rcloneNote}`;
  summary.style.cssText = 'color: #9aa3b2; font-size: 12px; margin-bottom: 8px;';
  content.appendChild(summary);

  // Note de ACCELA (si lo devolvió el endpoint) — sólo informativo,
  // no es error.
  if (accelaNote && extraAccela.length === 0) {
    const noteEl = doc.createElement('div');
    noteEl.textContent = `ℹ ${accelaNote}`;
    noteEl.style.cssText = 'color: #6b7280; font-size: 11px; margin-bottom: 8px;';
    content.appendChild(noteEl);
  }

  if (totalCount === 0) {
    const empty = doc.createElement('div');
    empty.textContent = 'Sin juegos configurados. Configura modos desde la GUI Iced.';
    empty.style.cssText = 'color: #6b7280; padding: 24px; text-align: center;';
    content.appendChild(empty);
    return;
  }

  // Lista unificada: registered games primero (sort alfabético), luego
  // ACCELA-only (también sort alfabético) marcados con tag distinto.
  const registered = [...resp.games].sort((a, b) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
  );
  const accelaOnly = [...extraAccela].sort((a, b) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
  );

  for (const game of registered) {
    content.appendChild(renderGameRow(doc, game));
  }
  for (const accela of accelaOnly) {
    content.appendChild(renderAccelaOnlyRow(doc, accela));
  }
}

/** Card de un juego: dot de status + nombre + descripción. */
function renderGameRow(doc: Document, game: any): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 12px',
    'padding: 10px 14px',
    'background: #171b26',
    'border: 1px solid #2a2f42',
    'border-radius: 6px',
  ].join(';');

  // Dot de status (color según status).
  const dot = doc.createElement('div');
  dot.style.cssText = [
    `background: ${statusColor(game.status)}`,
    'width: 10px',
    'height: 10px',
    'border-radius: 50%',
    'flex-shrink: 0',
  ].join(';');
  row.appendChild(dot);

  // Bloque de texto: nombre + descripción.
  const textBlock = doc.createElement('div');
  textBlock.style.cssText = 'display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0;';

  const name = doc.createElement('div');
  name.textContent = game.name;
  name.style.cssText = 'font-size: 13px; font-weight: 500; color: #ffffff;';
  textBlock.appendChild(name);

  const desc = doc.createElement('div');
  desc.textContent = describeGame(game);
  desc.style.cssText = 'font-size: 11px; color: #9aa3b2;';
  textBlock.appendChild(desc);

  row.appendChild(textBlock);
  return row;
}

/**
 * Card de un juego instalado por ACCELA pero no registrado todavía en
 * Ludusavi. Igual layout que renderGameRow pero con tag "📦 Installed
 * (ACCELA)" en gris en lugar de status dot, y sin descripción de sync.
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
    'background: #171b26',
    'border: 1px solid #2a2f42',
    'border-radius: 6px',
  ].join(';');

  // Dot gris para diferenciar (no hay status real, sólo "instalado en disco").
  const dot = doc.createElement('div');
  dot.style.cssText = [
    'background: #6b7280',
    'width: 10px',
    'height: 10px',
    'border-radius: 50%',
    'flex-shrink: 0',
  ].join(';');
  row.appendChild(dot);

  const textBlock = doc.createElement('div');
  textBlock.style.cssText =
    'display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0;';

  const name = doc.createElement('div');
  name.textContent = accela.name;
  name.style.cssText = 'font-size: 13px; font-weight: 500; color: #ffffff;';
  textBlock.appendChild(name);

  const desc = doc.createElement('div');
  const sizeStr = accela.size_on_disk > 0 ? ` · ${formatBytesShort(accela.size_on_disk)}` : '';
  desc.textContent = `📦 Installed (ACCELA)${sizeStr} · sin configuración de sync`;
  desc.style.cssText = 'font-size: 11px; color: #9aa3b2;';
  textBlock.appendChild(desc);

  row.appendChild(textBlock);
  return row;
}

/** Helper local: formatea bytes en forma compacta (KB/MB/GB). */
function formatBytesShort(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
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
 * Renderiza la tab "All Devices": lista todos los devices que el daemon
 * conoce, marcando cuál es el actual. Equivale a `Screen::AllDevices`
 * en la GUI Iced.
 *
 * Cada device se muestra como card con nombre, indicador de "this
 * device", último sync, y count de juegos. Sin click-to-expand por
 * ahora — el detalle de juegos por device viene cuando añadamos write
 * mode (Fase 2).
 */
async function loadAndRenderAllDevices(doc: Document, content: HTMLElement) {
  content.innerHTML = '';
  const loading = doc.createElement('div');
  loading.textContent = 'Cargando...';
  loading.style.cssText = 'color: #9aa3b2; font-size: 13px; padding: 12px;';
  content.appendChild(loading);

  let resp;
  try {
    resp = await daemon.getDevices();
  } catch (e) {
    showFatalError(doc, content, e);
    return;
  }

  content.innerHTML = '';

  const summary = doc.createElement('div');
  summary.textContent = `${resp.devices.length} device(s) conocido(s)`;
  summary.style.cssText = 'color: #9aa3b2; font-size: 12px; margin-bottom: 8px;';
  content.appendChild(summary);

  if (resp.devices.length === 0) {
    const empty = doc.createElement('div');
    empty.textContent =
      'No hay devices registrados. Ejecuta una operación de Ludusavi para inicializar.';
    empty.style.cssText = 'color: #6b7280; padding: 24px; text-align: center;';
    content.appendChild(empty);
    return;
  }

  // Sort: current device primero, luego resto alfabético.
  const sorted = [...resp.devices].sort((a, b) => {
    if (a.is_current && !b.is_current) return -1;
    if (!a.is_current && b.is_current) return 1;
    return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
  });

  for (const device of sorted) {
    content.appendChild(renderDeviceHeaderCard(doc, device, false));
  }
}

/**
 * Card de un device con icono + nombre + tag "this device" + last
 * sync + count de juegos. Usado tanto en la tab "This Device" (como
 * header destacado, expanded=true) como en "All Devices" (lista
 * compacta, expanded=false).
 */
function renderDeviceHeaderCard(
  doc: Document,
  device: ApiDeviceRow,
  expanded: boolean,
): HTMLElement {
  const card = doc.createElement('div');
  card.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 14px',
    expanded ? 'padding: 16px 18px' : 'padding: 12px 14px',
    'background: #171b26',
    device.is_current ? 'border: 1px solid #4f8ef7' : 'border: 1px solid #2a2f42',
    'border-radius: 6px',
    'margin-bottom: 6px',
  ].join(';');

  const icon = doc.createElement('div');
  icon.textContent = device.is_current ? '🖥' : '📡';
  icon.style.cssText = expanded ? 'font-size: 24px;' : 'font-size: 18px;';
  card.appendChild(icon);

  const textBlock = doc.createElement('div');
  textBlock.style.cssText =
    'display: flex; flex-direction: column; gap: 4px; flex: 1; min-width: 0;';

  // Línea 1: nombre + tag "this device" si aplica.
  const nameRow = doc.createElement('div');
  nameRow.style.cssText = 'display: flex; align-items: center; gap: 8px;';
  const name = doc.createElement('div');
  name.textContent = device.name;
  name.style.cssText = expanded
    ? 'font-size: 15px; font-weight: 600; color: #ffffff;'
    : 'font-size: 13px; font-weight: 500; color: #ffffff;';
  nameRow.appendChild(name);
  if (device.is_current) {
    const tag = doc.createElement('div');
    tag.textContent = 'this device';
    tag.style.cssText = [
      'font-size: 10px',
      'color: #4f8ef7',
      'border: 1px solid #4f8ef7',
      'padding: 1px 6px',
      'border-radius: 8px',
      'text-transform: uppercase',
      'letter-spacing: 0.05em',
    ].join(';');
    nameRow.appendChild(tag);
  }
  textBlock.appendChild(nameRow);

  // Línea 2: stats — last sync · N juegos.
  const stats = doc.createElement('div');
  const lastSync = relativeTime(device.last_sync_time_utc);
  const gamesCount = device.games.length;
  stats.textContent = `${gamesCount} juego(s) · last sync: ${lastSync}`;
  stats.style.cssText = 'font-size: 11px; color: #9aa3b2;';
  textBlock.appendChild(stats);

  card.appendChild(textBlock);
  return card;
}

/** Fila simple con sólo el nombre del juego — fallback cuando no
 *  tenemos el detalle de /api/games. */
function renderPlainNameRow(doc: Document, name: string): HTMLElement {
  const row = doc.createElement('div');
  row.style.cssText = [
    'display: flex',
    'align-items: center',
    'gap: 12px',
    'padding: 10px 14px',
    'background: #171b26',
    'border: 1px solid #2a2f42',
    'border-radius: 6px',
  ].join(';');

  const dot = doc.createElement('div');
  dot.style.cssText =
    'background: #6b7280; width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;';
  row.appendChild(dot);

  const label = doc.createElement('div');
  label.textContent = name;
  label.style.cssText = 'font-size: 13px; color: #ffffff;';
  row.appendChild(label);
  return row;
}

/** Renderiza un mensaje de error fatal en `content` y limpia el resto. */
function showFatalError(doc: Document, content: HTMLElement, e: unknown) {
  content.innerHTML = '';
  const err = doc.createElement('div');
  err.textContent = `✗ ${e}`;
  err.style.cssText = 'color: #ef4444; font-size: 13px; padding: 12px;';
  content.appendChild(err);
}

/** Oculta el overlay si está visible (sin destruirlo — visibility toggle).
 *  Cierra el EventSource para no dejar conexiones abiertas mientras el
 *  usuario está en otra pestaña de Steam. */
function hideSyncOverlay(doc: Document) {
  const overlay = doc.querySelector<HTMLElement>(`[${OVERLAY_ATTR}]`);
  if (overlay) {
    overlay.style.display = 'none';
  }
  disconnectSse();
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
async function connectSse(
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
  }
}

/** Cierra el EventSource activo y cancela cualquier refresh pendiente. */
function disconnectSse() {
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
