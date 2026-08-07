export function gameAccent(name: string): { hue: number; initials: string } {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  const hue = Math.abs(hash) % 360;
  const initials = name
    .split(/\s+/)
    .map((w) => w[0] ?? '')
    .join('')
    .slice(0, 2)
    .toUpperCase();
  return { hue, initials: initials || '?' };
}

/** Format stored playtime_hours for UI (e.g. "12.4 h", "42 m"). */
export function formatPlaytime(hours: number | null | undefined): string | null {
  if (hours == null || hours <= 0) return null;
  if (hours < 1) {
    const mins = Math.max(1, Math.round(hours * 60));
    return `${mins} m`;
  }
  return `${hours.toFixed(hours < 10 ? 1 : 0)} h`;
}

/** Normalize scan source for UI labels (Steam / Epic / Custom / …). */
export function formatSource(source: string | null | undefined): string {
  const raw = (source ?? 'custom').trim().toLowerCase();
  if (raw === 'steam') return 'Steam';
  if (raw === 'epic' || raw === 'egs') return 'Epic';
  if (raw === 'custom') return 'Custom';
  if (raw === 'gog') return 'GOG';
  if (raw === 'xbox' || raw === 'msstore') return 'Xbox';
  if (!raw) return 'Custom';
  return raw.charAt(0).toUpperCase() + raw.slice(1);
}
