import { useEffect, useState } from 'react';
import { launcherInvoke } from '../launcher-bridge';
import { CloudSyncSettings } from './CloudSyncSettings';

type Props = {
  onRefresh: () => void;
  onSteamKeySaved?: (configured: boolean) => void;
  onCoversKeySaved?: () => void;
};

type Settings = {
  steamApiKey: string;
  steamGridDbApiKey: string;
  overlayDebug: boolean;
};

export function SettingsView({
  onRefresh,
  onSteamKeySaved,
  onCoversKeySaved,
}: Props) {
  const catalogUrl =
    import.meta.env.VITE_STORE_CATALOG ??
    'config/store-catalog.json (local default)';
  const [steamApiKey, setSteamApiKey] = useState('');
  const [steamGridDbApiKey, setSteamGridDbApiKey] = useState('');
  const [overlayDebug, setOverlayDebug] = useState(false);
  const [saved, setSaved] = useState<'steam' | 'sgdb' | 'debug' | null>(null);
  const [saving, setSaving] = useState(false);
  const [logsDir, setLogsDir] = useState<string | null>(null);

  useEffect(() => {
    void launcherInvoke<Settings>('settings.get')
      .then((s) => {
        setSteamApiKey(s.steamApiKey ?? '');
        setSteamGridDbApiKey(s.steamGridDbApiKey ?? '');
        setOverlayDebug(Boolean(s.overlayDebug));
      })
      .catch(() => null);
  }, []);

  const save = async (kind: 'steam' | 'sgdb' | 'debug') => {
    setSaving(true);
    try {
      await launcherInvoke<Settings>('settings.set', [
        { steamApiKey, steamGridDbApiKey, overlayDebug },
      ]);
      setSaved(kind);
      if (kind === 'steam') onSteamKeySaved?.(Boolean(steamApiKey.trim()));
      if (kind === 'sgdb') onCoversKeySaved?.();
      setTimeout(() => setSaved(null), 2000);
    } finally {
      setSaving(false);
    }
  };

  const openLogs = async () => {
    const res = await launcherInvoke<{ ok: boolean; dir: string }>('logs.openDir');
    setLogsDir(res.dir);
  };

  return (
    <div>
      <p className="page-eyebrow">Preferences</p>
      <h1 className="page-title">Settings</h1>

      <CloudSyncSettings />

      <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
        <label htmlFor="sgdb-api-key">SteamGridDB API key</label>
        <input
          id="sgdb-api-key"
          type="password"
          autoComplete="off"
          placeholder="From steamgriddb.com preferences"
          value={steamGridDbApiKey}
          onChange={(e) => setSteamGridDbApiKey(e.target.value)}
        />
        <p className="settings-hint">
          Used for library covers (600×900 grids), same approach as{' '}
          <a
            href="https://github.com/cooperate/SteamGridDBMetadata"
            target="_blank"
            rel="noreferrer"
          >
            SteamGridDBMetadata
          </a>
          .
        </p>
        <button
          type="button"
          className="btn-primary"
          disabled={saving}
          onClick={() => void save('sgdb')}
        >
          {saving ? 'Saving…' : saved === 'sgdb' ? 'Saved' : 'Save GridDB key'}
        </button>
      </div>

      <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
        <label htmlFor="steam-api-key">Steam Web API key</label>
        <input
          id="steam-api-key"
          type="password"
          autoComplete="off"
          placeholder="Paste key from steamcommunity.com/dev/apikey"
          value={steamApiKey}
          onChange={(e) => setSteamApiKey(e.target.value)}
        />
        <p className="settings-hint">
          Used later for achievements. Stored in{' '}
          <code>%APPDATA%/Glint/launcher-settings.json</code>.
        </p>
        <button
          type="button"
          className="btn-primary"
          disabled={saving}
          onClick={() => void save('steam')}
        >
          {saving ? 'Saving…' : saved === 'steam' ? 'Saved' : 'Save Steam key'}
        </button>
      </div>

      <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
        <label htmlFor="overlay-debug">
          <input
            id="overlay-debug"
            type="checkbox"
            checked={overlayDebug}
            onChange={(e) => setOverlayDebug(e.target.checked)}
            style={{ marginRight: 8 }}
          />
          Overlay / CEF debug logging
        </label>
        <p className="settings-hint">
          Writes <code>[CEF-STAGE]</code> lines into{' '}
          <code>%LOCALAPPDATA%\Glint\logs\overlay-*.log</code> on the next
          attach (overlay host has no console window). Enable, Save, then relaunch
          the game / re-attach overlay.
        </p>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button
            type="button"
            className="btn-primary"
            disabled={saving}
            onClick={() => void save('debug')}
          >
            {saving ? 'Saving…' : saved === 'debug' ? 'Saved' : 'Save debug setting'}
          </button>
          <button type="button" className="btn-secondary" onClick={() => void openLogs()}>
            Open logs folder
          </button>
        </div>
        {logsDir ? (
          <p className="settings-hint" style={{ marginTop: 8 }}>
            Opened: <code>{logsDir}</code>
          </p>
        ) : null}
      </div>

      <div className="settings-section glass" style={{ padding: 20 }}>
        <label htmlFor="catalog-url">Store catalog URL</label>
        <input id="catalog-url" readOnly value={catalogUrl} />
        <p className="settings-hint">
          Override with environment variable{' '}
          <code>GLINT_STORE_CATALOG</code>. Installed apps live in{' '}
          <code>%APPDATA%/Glint/apps/</code>.
        </p>
        <button type="button" className="btn-secondary" onClick={onRefresh}>
          Refresh library &amp; store
        </button>
      </div>
    </div>
  );
}
