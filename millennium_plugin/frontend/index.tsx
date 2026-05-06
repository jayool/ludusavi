/**
 * Ludusavi Sync — plugin Millennium (Fase 1).
 *
 * Estructura: valida primero la conexión al daemon HTTP, y si todo OK
 * renderiza la tabla de juegos (read-only). Las pantallas This Device
 * y All Devices llegan en commits sucesivos.
 */

import { definePlugin, IconsModule, Field, DialogButton } from '@steambrew/client';
import { daemon, DaemonStatus } from './daemon-client';
import { GamesTable } from './games-table';
import { runDomProbe, injectTestSyncTab, autoInjectSyncTabOnLoad } from './dom-probe';

type ConnState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ok'; status: DaemonStatus }
  | { kind: 'err'; message: string };

type DomProbeState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'done'; report: string };

const SyncTab = () => {
  const SP_REACT = (window as any).SP_REACT;
  const [conn, setConn] = SP_REACT.useState<ConnState>({ kind: 'idle' });
  const [probe, setProbe] = SP_REACT.useState<DomProbeState>({ kind: 'idle' });

  const probeConn = async () => {
    setConn({ kind: 'loading' });
    daemon.clearToken();
    try {
      const status = await daemon.getStatus();
      setConn({ kind: 'ok', status });
    } catch (e) {
      setConn({ kind: 'err', message: String(e) });
    }
  };

  const runProbe = async () => {
    setProbe({ kind: 'running' });
    try {
      const report = await runDomProbe();
      setProbe({ kind: 'done', report });
    } catch (e) {
      setProbe({ kind: 'done', report: `[ERROR] ${e}` });
    }
  };

  SP_REACT.useEffect(() => {
    probeConn();
  }, []);

  return (
    <>
      <Field
        label="Conexión con el daemon"
        description={(() => {
          switch (conn.kind) {
            case 'idle':
              return 'Idle.';
            case 'loading':
              return 'Conectando...';
            case 'ok':
              return `✓ daemon v${conn.status.version} (api v${conn.status.api_version})`;
            case 'err':
              return `✗ ${conn.message}`;
          }
        })()}
        focusable
      >
        <DialogButton onClick={probeConn}>Retry</DialogButton>
      </Field>

      {conn.kind === 'err' && (
        <Field
          label="Cómo arreglar"
          description={
            'Asegúrate de que: (1) el daemon está corriendo, (2) usa la versión con HTTP API ' +
            '(rama claude/daemon-http-api o merge), (3) %APPDATA%\\ludusavi\\daemon-token.txt existe.'
          }
        />
      )}

      {/* Cuando la conexión está OK, renderizamos la tabla. Si falla
          la tabla mostrará su propio error sin afectar al check de
          conexión de arriba. */}
      {conn.kind === 'ok' && <GamesTable />}

      {/* DOM probe del main window para planear la inyección de la
          pestaña SYNC. Renderiza el report aquí mismo para evitar
          tener que abrir DevTools. */}
      <Field
        label="DOM probe (main window de Steam)"
        description={(() => {
          switch (probe.kind) {
            case 'idle':
              return 'Pulsa "Run probe" para que el plugin inspeccione el DOM del main window de Steam y muestre aquí mismo qué selectores funcionan, dónde está la nav, etc.';
            case 'running':
              return 'Probing... (espera ~5s a que el main window esté listo)';
            case 'done':
              return 'Report listo abajo. Selecciona y copia.';
          }
        })()}
        focusable
      >
        <DialogButton onClick={runProbe}>Run probe</DialogButton>
      </Field>

      {probe.kind === 'done' && (
        <Field
          label="Probe report"
          description={probe.report}
          focusable
        />
      )}

      <SyncTabInjector />
    </>
  );
};

/** Botón aparte para inyectar un SYNC tab de prueba en la nav principal. */
const SyncTabInjector = () => {
  const SP_REACT = (window as any).SP_REACT;
  const [result, setResult] = SP_REACT.useState<string>('');

  const inject = async () => {
    setResult('Inyectando...');
    try {
      const r = await injectTestSyncTab();
      setResult(r);
    } catch (e) {
      setResult(`[ERROR] ${e}`);
    }
  };

  return (
    <>
      <Field
        label="Inyectar SYNC tab (test)"
        description={
          'Pulsa para inyectar un botón "SYNC" físico en la barra superior ' +
          'de Steam, junto a Biblioteca/Tienda/Comunidad. Por ahora el click ' +
          'sólo parpadea verde — el handler real llega cuando confirmes que el ' +
          'botón se ve bien y en el sitio correcto.'
        }
        focusable
      >
        <DialogButton onClick={inject}>Inject SYNC tab</DialogButton>
      </Field>
      {result && <Field label="Resultado" description={result} focusable />}
    </>
  );
};

export default definePlugin(() => {
  // Auto-inyectar el SYNC tab en la nav principal de Steam al cargar
  // el plugin. Función no-async: arranca dos vías (polling + hook de
  // window-create) en background. Ver autoInjectSyncTabOnLoad().
  autoInjectSyncTabOnLoad();

  return {
    title: 'Ludusavi Sync',
    icon: <IconsModule.Settings />,
    content: <SyncTab />,
  };
});
