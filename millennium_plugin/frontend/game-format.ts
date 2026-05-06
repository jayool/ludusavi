/**
 * Helpers de formato para los juegos del daemon. Compartido entre la
 * `<GamesTable>` (settings panel) y el overlay del SYNC tab (main
 * window vanilla DOM).
 */

import { ApiGameRow } from './daemon-client';
import { relativeTime } from './time-format';

export function modeBadge(mode: string): string {
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

export function statusBadge(status: string): string {
  switch (status) {
    case 'synced':
      return '✓ synced';
    case 'pending_backup':
      return '⏳ pending backup';
    case 'pending_restore':
      return '⏳ pending restore';
    case 'error':
      return '✗ error';
    case 'conflict':
      return '⚠ conflict';
    case 'not_managed':
      return '— not managed';
    case '':
      return '';
    default:
      return status;
  }
}

/**
 * Color hex apropiado para el status, en la paleta del fork.
 * Útil para renderizar el dot de status (en el overlay vanilla DOM).
 */
export function statusColor(status: string): string {
  switch (status) {
    case 'synced':
      return '#3ecf8e'; // verde
    case 'pending_backup':
    case 'pending_restore':
      return '#f0b400'; // amarillo
    case 'error':
    case 'conflict':
      return '#ef4444'; // rojo
    default:
      return '#6b7280'; // gris
  }
}

/** Construye la línea de descripción para una fila. */
export function describeGame(game: ApiGameRow): string {
  const parts: string[] = [];
  parts.push(modeBadge(game.mode));

  const status = statusBadge(game.status);
  if (status) parts.push(status);

  if (game.mode === 'sync' && game.last_synced_from) {
    const when = relativeTime(game.last_sync_time_utc);
    parts.push(`${when} from ${game.last_synced_from}`);
  } else if (game.last_sync_time_utc) {
    parts.push(relativeTime(game.last_sync_time_utc));
  }

  if (game.error) {
    parts.push(`[${game.error.category}/${game.error.direction}: ${game.error.message}]`);
  }
  if (game.conflict) {
    parts.push(`[conflict with ${game.conflict.cloud_from || 'cloud'}]`);
  }

  return parts.join(' · ');
}
