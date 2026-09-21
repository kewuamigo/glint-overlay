import { getDb, getSetting, setSetting } from './db.js';

export type LauncherSettings = {
  steamApiKey: string;
  steamGridDbApiKey: string;
  /** When true, overlay host gets GLINT_DEBUG=1 (CEF stage logs → log file). */
  overlayDebug: boolean;
  /** When true, launcher checks GitHub Releases after startup (24h interval). */
  otaAutoCheck: boolean;
};

const DEFAULTS: LauncherSettings = {
  steamApiKey: '',
  steamGridDbApiKey: '',
  overlayDebug: false,
  otaAutoCheck: true,
};

export function loadSettings(): LauncherSettings {
  getDb();
  return {
    steamApiKey: getSetting('steamApiKey') ?? DEFAULTS.steamApiKey,
    steamGridDbApiKey:
      getSetting('steamGridDbApiKey') ?? DEFAULTS.steamGridDbApiKey,
    overlayDebug: getSetting('overlayDebug') === '1',
    otaAutoCheck: getSetting('otaAutoCheck') !== '0',
  };
}

export function saveSettings(partial: Partial<LauncherSettings>): LauncherSettings {
  getDb();
  if (typeof partial.steamApiKey === 'string') {
    setSetting('steamApiKey', partial.steamApiKey);
  }
  if (typeof partial.steamGridDbApiKey === 'string') {
    setSetting('steamGridDbApiKey', partial.steamGridDbApiKey);
  }
  if (typeof partial.overlayDebug === 'boolean') {
    setSetting('overlayDebug', partial.overlayDebug ? '1' : '0');
  }
  if (typeof partial.otaAutoCheck === 'boolean') {
    setSetting('otaAutoCheck', partial.otaAutoCheck ? '1' : '0');
  }
  return loadSettings();
}
