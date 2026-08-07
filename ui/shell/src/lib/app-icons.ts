const FALLBACK_ICONS: Record<string, string> = {
  browser: '🌐',
  metrics: '📊',
  achievements: '🏆',
};

export function getAppFallbackIcon(manifestId: string, title: string): string {
  return FALLBACK_ICONS[manifestId] ?? title.charAt(0).toUpperCase();
}
