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
import { daemon } from './daemon-client';
import { describeGame, statusColor } from './game-format';

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

/**
 * Muestra el overlay del SYNC tab dentro del main window de Steam.
 * Idempotente: si ya está visible recarga la lista.
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
  await loadAndRenderGames(doc, content);
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
    loadAndRenderGames(doc, content);
  });
  header.appendChild(refreshBtn);

  overlay.appendChild(header);

  // Content area que se rellena async con loadAndRenderGames.
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
    content.innerHTML = '';
    const err = doc.createElement('div');
    err.textContent = `✗ ${gamesSettled.reason}`;
    err.style.cssText = 'color: #ef4444; font-size: 13px; padding: 12px;';
    content.appendChild(err);
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

/** Oculta el overlay si está visible (sin destruirlo — visibility toggle). */
function hideSyncOverlay(doc: Document) {
  const overlay = doc.querySelector<HTMLElement>(`[${OVERLAY_ATTR}]`);
  if (overlay) {
    overlay.style.display = 'none';
  }
}
