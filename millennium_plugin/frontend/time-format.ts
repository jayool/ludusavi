/**
 * Helpers de formato de tiempo para la UI del plugin.
 *
 * El daemon devuelve timestamps en RFC 3339 UTC (ej.
 * "2026-05-06T12:00:00Z"). Aquí los pasamos a strings relativos
 * legibles que la UI puede mostrar directamente.
 */

/**
 * Devuelve un texto relativo tipo "3m ago", "2h ago", "5d ago",
 * o "Never" si no hay timestamp. Útil para columnas de sync.
 */
export function relativeTime(iso?: string): string {
  if (!iso) return 'Never';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso; // No podemos parsearlo, lo mostramos crudo
  const now = Date.now();
  const diffSec = Math.max(0, Math.floor((now - t) / 1000));

  if (diffSec < 60) return 'just now';
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDay = Math.floor(diffHr / 24);
  if (diffDay < 30) return `${diffDay}d ago`;
  const diffMonth = Math.floor(diffDay / 30);
  if (diffMonth < 12) return `${diffMonth}mo ago`;
  return `${Math.floor(diffMonth / 12)}y ago`;
}

/**
 * Formato human-readable de tamaño de fichero.
 * 1024 → "1 KB", 1048576 → "1 MB", etc.
 */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const idx = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  const value = bytes / Math.pow(1024, idx);
  return `${value.toFixed(value >= 100 || idx === 0 ? 0 : 1)} ${units[idx]}`;
}
