/**
 * Ruta personalizada `/ludusavi-sync` registrada en el router de
 * Steam. Cuando navegamos a ella (via SteamUIStore.Navigate o
 * MainWindowBrowserManager.ShowURL), Steam renderiza este componente
 * en el área de contenido principal — donde aparece Biblioteca,
 * Tienda, etc. Sin popups, sin overlays DOM, sin pelear con
 * compositores: nuestro contenido toma el lugar del contenido del
 * tab anterior dentro de la misma ventana.
 *
 * Pattern: routerHook.addRoute(path, component) + SteamUIStore.Navigate
 * — visto en luthor112/steam-librarian para
 * `SteamUIStore.Navigate("/millennium/settings")` y en la API de
 * @steambrew/client.
 */

import { routerHook } from '@steambrew/client';
import {
  buildOverlayShell,
  connectSse,
  disconnectSse,
  renderActiveTab,
} from './dom-probe';

declare const SteamUIStore: any;

const SP_REACT = (window as any).SP_REACT;

/** Path bajo el que se monta nuestra UI en la nav React de Steam.
 *  Tiene que empezar por "/" para que el router lo reconozca. */
export const SYNC_ROUTE_PATH = '/ludusavi-sync';

/** Componente que React monta cuando Steam navega a SYNC_ROUTE_PATH.
 *  Crea un contenedor DOM y mete dentro nuestro shell vanilla
 *  (header + tabs + content) usando los renderers existentes. La
 *  conversión a React 100% se queda como deuda — esto basta para
 *  que la UI viva dentro de Steam sin pelear con stacking. */
function SyncRoute() {
  const containerRef = SP_REACT.useRef(null);

  SP_REACT.useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const doc = container.ownerDocument || document;
    const shell = buildOverlayShell(doc);
    container.appendChild(shell);

    const content = shell.querySelector('[data-content]') as HTMLElement;
    const statusPill = shell.querySelector(
      '[data-sse-status]',
    ) as HTMLElement;

    renderActiveTab(doc, shell, content);
    connectSse(doc, shell, content, statusPill);

    return () => {
      // Cleanup al navegar fuera de la ruta — Steam desmonta el
      // componente y aquí cerramos SSE / vaciamos el contenedor.
      disconnectSse();
      if (container) container.innerHTML = '';
    };
  }, []);

  return SP_REACT.createElement('div', {
    ref: containerRef,
    style: { width: '100%', height: '100%' },
  });
}

/** Registramos la ruta al cargar el módulo. Tiene que ocurrir antes
 *  de que el usuario intente navegar; importar este fichero desde
 *  index.tsx garantiza que esto corra al cargar el plugin. */
routerHook.addRoute(SYNC_ROUTE_PATH, SyncRoute);

/** Helper que dispara la navegación de Steam a nuestra ruta. Lo
 *  llama el click handler de la SYNC tab en dom-probe.ts. */
export function navigateToSyncRoute() {
  if (typeof SteamUIStore !== 'undefined' && SteamUIStore?.Navigate) {
    try {
      SteamUIStore.Navigate(SYNC_ROUTE_PATH);
      return;
    } catch (e) {
      console.error('[ludusavi-sync] SteamUIStore.Navigate threw:', e);
    }
  }
  console.error(
    '[ludusavi-sync] SteamUIStore.Navigate no disponible — la ruta no se puede abrir',
  );
}
