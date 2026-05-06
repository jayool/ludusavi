/**
 * Popup React-nativo para la pestaña SYNC. Usa
 * `routerHook.addGlobalComponent` y el `GenericDialog` interno de
 * Steam para renderizar UN POPUP NATIVO de Steam — gestionado por
 * Steam, parte de su árbol React, **encima de cualquier ventana
 * incluyendo Tienda y Comunidad** (que viven en procesos CEF
 * separados).
 *
 * Este patrón está copiado del plugin Extendium
 * (https://github.com/BossSloth/Extendium) que tuvo que resolver
 * exactamente el mismo problema (poner UI propia encima de Tienda).
 *
 * Antes peleábamos con z-index, transform, hide-iframes, etc. —
 * todo inútil porque los procesos CEF de Tienda se componen al nivel
 * del SO y NADA en nuestro DOM puede taparlos. Esta es la única vía
 * fiable: dejar que Steam dibuje el popup como propio.
 */

import { findModuleExport, ModalRoot, routerHook } from '@steambrew/client';
import { renderOverlayBody } from './dom-probe';

const SP_REACT = (window as any).SP_REACT;
const { useState, useEffect, useRef } = SP_REACT;

// Modos de UI de Steam — Desktop = 7. Coincide con la enum
// `EUIMode.Desktop` de @steambrew/client (no importamos la enum
// directamente porque está en un sub-path que el bundler tiene a
// veces problemas para resolver).
const EUI_MODE_DESKTOP = 7;

/**
 * Encuentra el `GenericDialog` interno de Steam — el componente
 * React que Steam usa para sus propios popups (Settings, Friends,
 * etc.). Buscar por características únicas de su signature
 * (props popupHeight/popupWidth/onlyPopoutIfNeeded). Mismo método
 * que usan otros plugins (Extendium, etc.).
 */
const GenericDialog: any = findModuleExport(
  (e: any) =>
    typeof e?.toString === 'function' &&
    e.toString().includes('.popupHeight') &&
    e.toString().includes('.popupWidth') &&
    e.toString().includes('.onlyPopoutIfNeeded'),
);

// Mecanismo simple de pub/sub para abrir/cerrar el popup desde
// fuera del componente React (lo llama el click handler de la
// SYNC tab que vive en dom-probe.ts).
let listeners: Array<(open: boolean) => void> = [];

export function openSyncPopup() {
  listeners.forEach((l) => l(true));
}

export function closeSyncPopup() {
  listeners.forEach((l) => l(false));
}

function SyncPopupComponent() {
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement | null>(null);

  // Registrar/desregistrar listener al mount/unmount.
  useEffect(() => {
    listeners.push(setOpen);
    return () => {
      listeners = listeners.filter((l) => l !== setOpen);
    };
  }, []);

  // Cuando el popup pasa a abierto, montamos el body via la función
  // de dom-probe.ts que construye header + tabs + content area en
  // vanilla DOM (reusamos la lógica existente para no duplicar).
  useEffect(() => {
    if (!open) return;
    if (!containerRef.current) return;
    renderOverlayBody(containerRef.current);
  }, [open]);

  if (!open) return null;
  if (!GenericDialog) {
    console.error(
      '[ludusavi-sync] GenericDialog no encontrado — Steam version no soportada?',
    );
    return null;
  }

  // Calculamos tamaño máximo. Steam dialog respeta popupWidth/Height
  // pero queda centrado. Para que parezca fullscreen usamos
  // dimensiones cercanas a la ventana entera.
  const w = window.innerWidth;
  const h = window.innerHeight - 60;

  return SP_REACT.createElement(
    GenericDialog,
    {
      strTitle: 'Ludusavi Sync',
      onDismiss: () => setOpen(false),
      popupWidth: w,
      popupHeight: h,
      modal: true, // dentro de la ventana de Steam, no popup separado
      resizable: false,
    },
    SP_REACT.createElement(
      ModalRoot,
      { onCancel: () => setOpen(false) },
      SP_REACT.createElement('div', {
        ref: containerRef,
        style: {
          width: '100%',
          height: '100%',
          minHeight: '500px',
          background: '#0f1117',
          color: '#ffffff',
          fontFamily: '"Motiva Sans", sans-serif',
        },
      }),
    ),
  );
}

// Registrar como componente global. Steam llamará a
// `<SyncPopupComponent />` dentro de su árbol React, lo que hace que
// el dialog flote por encima de cualquier ventana — incluso Tienda
// y Comunidad — porque es parte de Steam, no de un overlay DOM
// nuestro.
routerHook.addGlobalComponent(
  'LudusaviSyncPopup',
  SyncPopupComponent,
  EUI_MODE_DESKTOP,
);
