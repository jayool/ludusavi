/**
 * Componente de tabla de juegos del plugin Ludusavi Sync.
 *
 * Llama a `daemon.getGames()` al montar y renderiza una fila por juego
 * usando el componente nativo `Field` de Millennium. Read-only: en
 * Fase 1 sólo se muestra info; los selectores de modo y los botones
 * Sync/Backup/Restore vienen en una fase posterior cuando el daemon
 * exponga endpoints WRITE.
 */

import { Field, DialogButton } from '@steambrew/client';
import { daemon, ApiGameRow } from './daemon-client';
import { describeGame } from './game-format';

type LoadState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ok'; games: ApiGameRow[]; deviceName: string; rcloneMissing: boolean }
  | { kind: 'err'; message: string };

interface GamesTableProps {
  /** Disparado al pulsar "Refresh" — útil si el padre quiere también
   *  refrescar otras secciones simultáneamente. Si null, internamente
   *  refresca solo la tabla. */
  onRefresh?: () => void;
}

export const GamesTable = (_props: GamesTableProps) => {
  const SP_REACT = (window as any).SP_REACT;
  const [state, setState] = SP_REACT.useState<LoadState>({ kind: 'idle' });

  const load = async () => {
    setState({ kind: 'loading' });
    try {
      const resp = await daemon.getGames();
      // Orden alfabético por nombre (case-insensitive). El daemon devuelve
      // ordenado pero no aseguramos el contrato — defensivo aquí.
      const games = [...resp.games].sort((a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
      );
      setState({
        kind: 'ok',
        games,
        deviceName: resp.device.name,
        rcloneMissing: resp.rclone_missing,
      });
    } catch (e) {
      setState({ kind: 'err', message: String(e) });
    }
  };

  SP_REACT.useEffect(() => {
    load();
  }, []);

  return (
    <>
      <Field
        label="Games"
        description={(() => {
          switch (state.kind) {
            case 'idle':
              return 'Idle.';
            case 'loading':
              return 'Cargando lista de juegos...';
            case 'ok':
              return `${state.games.length} juego(s) — device: ${state.deviceName}${state.rcloneMissing ? ' · ⚠ rclone no disponible' : ''}`;
            case 'err':
              return `✗ ${state.message}`;
          }
        })()}
        focusable
      >
        <DialogButton onClick={load}>Refresh</DialogButton>
      </Field>

      {state.kind === 'ok' && state.games.length === 0 && (
        <Field
          label="Sin juegos"
          description="No hay juegos en sync-games.json ni en el game-list del cloud. Configura algunos desde la GUI Iced."
        />
      )}

      {state.kind === 'ok' &&
        state.games.map((g) => (
          <Field key={g.name} label={g.name} description={describeGame(g)} focusable />
        ))}
    </>
  );
};
