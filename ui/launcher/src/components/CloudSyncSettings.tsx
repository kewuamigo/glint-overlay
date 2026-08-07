import { useEffect, useState } from 'react';
import {
  launcherInvoke,
  type CloudStorageConfig,
  type CloudStorageState,
} from '../launcher-bridge';

type ProviderId = CloudStorageConfig['provider'];

type Draft = {
  provider: ProviderId;
  baseUrl: string;
  bearerToken: string;
  host: string;
  user: string;
  password: string;
  basePath: string;
  secure: boolean;
  port: string;
  accessToken: string;
  folderId: string;
};

const emptyDraft = (provider: ProviderId = 'selfhost'): Draft => ({
  provider,
  baseUrl: '',
  bearerToken: '',
  host: '',
  user: '',
  password: '',
  basePath: '/',
  secure: true,
  port: '',
  accessToken: '',
  folderId: '',
});

function draftFromConfig(config: CloudStorageConfig | null): Draft {
  if (!config) return emptyDraft();
  if (config.provider === 'selfhost') {
    return {
      ...emptyDraft('selfhost'),
      baseUrl: config.baseUrl,
      bearerToken: config.bearerToken,
    };
  }
  if (config.provider === 'ftp') {
    return {
      ...emptyDraft('ftp'),
      host: config.host,
      user: config.user,
      password: config.password,
      basePath: config.basePath,
      secure: config.secure,
      port: config.port != null ? String(config.port) : '',
    };
  }
  return {
    ...emptyDraft('drive'),
    accessToken: config.accessToken,
    folderId: config.folderId ?? '',
  };
}

function configFromDraft(d: Draft): CloudStorageConfig {
  if (d.provider === 'selfhost') {
    return {
      provider: 'selfhost',
      baseUrl: d.baseUrl.trim(),
      bearerToken: d.bearerToken.trim(),
    };
  }
  if (d.provider === 'ftp') {
    const port = d.port.trim() ? Number(d.port) : undefined;
    return {
      provider: 'ftp',
      host: d.host.trim(),
      user: d.user.trim(),
      password: d.password,
      basePath: d.basePath.trim() || '/',
      secure: d.secure,
      ...(port && !Number.isNaN(port) ? { port } : {}),
    };
  }
  return {
    provider: 'drive',
    accessToken: d.accessToken.trim(),
    ...(d.folderId.trim() ? { folderId: d.folderId.trim() } : {}),
  };
}

export function CloudSyncSettings() {
  const [enabled, setEnabled] = useState(false);
  const [draft, setDraft] = useState<Draft>(emptyDraft());
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    void launcherInvoke<CloudStorageState>('cloud.storage.getConfig')
      .then((s) => {
        setEnabled(s.enabled === true);
        setDraft(draftFromConfig(s.config));
      })
      .catch(() => null);
  }, []);

  const patch = (p: Partial<Draft>) => setDraft((d) => ({ ...d, ...p }));

  const toggleEnabled = async (next: boolean) => {
    setBusy(true);
    setErr(null);
    try {
      const s = await launcherInvoke<CloudStorageState>('cloud.sync.setEnabled', [
        next,
      ]);
      setEnabled(s.enabled === true);
      setMsg(next ? 'Cloud sync enabled' : 'Cloud sync disabled');
      setTimeout(() => setMsg(null), 2000);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const saveConfig = async () => {
    setBusy(true);
    setErr(null);
    try {
      const cfg = configFromDraft(draft);
      const s = await launcherInvoke<CloudStorageState>(
        'cloud.storage.setConfig',
        [cfg],
      );
      setDraft(draftFromConfig(s.config));
      setMsg('Provider saved');
      setTimeout(() => setMsg(null), 2000);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const testConnection = async () => {
    setBusy(true);
    setErr(null);
    try {
      const cfg = configFromDraft(draft);
      const res = await launcherInvoke<{ ok: true; folderId?: string }>(
        'cloud.storage.testConnection',
        [cfg],
      );
      if (res.folderId) {
        patch({ folderId: res.folderId });
      }
      setMsg(
        res.folderId
          ? `Connection OK (Drive folder ${res.folderId})`
          : 'Connection OK',
      );
      setTimeout(() => setMsg(null), 3000);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
      <label htmlFor="cloud-sync-enabled">
        <input
          id="cloud-sync-enabled"
          type="checkbox"
          checked={enabled}
          disabled={busy}
          onChange={(e) => void toggleEnabled(e.target.checked)}
          style={{ marginRight: 8 }}
        />
        Enable cloud sync
      </label>
      <p className="settings-hint">
        When enabled, the launcher uploads saves and achievements after a tracked
        game exits, and can restore from Library featured game actions.
      </p>

      <label htmlFor="cloud-provider">Storage provider</label>
      <select
        id="cloud-provider"
        value={draft.provider}
        disabled={busy}
        onChange={(e) =>
          patch({ provider: e.target.value as ProviderId })
        }
      >
        <option value="selfhost">Self-host</option>
        <option value="ftp">FTP / FTPS</option>
        <option value="drive">Google Drive</option>
      </select>

      {draft.provider === 'selfhost' && (
        <>
          <label htmlFor="cloud-base-url">Base URL</label>
          <input
            id="cloud-base-url"
            type="url"
            autoComplete="off"
            placeholder="https://sync.example.com"
            value={draft.baseUrl}
            onChange={(e) => patch({ baseUrl: e.target.value })}
          />
          <label htmlFor="cloud-bearer">Bearer token</label>
          <input
            id="cloud-bearer"
            type="password"
            autoComplete="off"
            value={draft.bearerToken}
            onChange={(e) => patch({ bearerToken: e.target.value })}
          />
        </>
      )}

      {draft.provider === 'ftp' && (
        <>
          <label htmlFor="cloud-ftp-host">Host</label>
          <input
            id="cloud-ftp-host"
            autoComplete="off"
            value={draft.host}
            onChange={(e) => patch({ host: e.target.value })}
          />
          <label htmlFor="cloud-ftp-user">User</label>
          <input
            id="cloud-ftp-user"
            autoComplete="off"
            value={draft.user}
            onChange={(e) => patch({ user: e.target.value })}
          />
          <label htmlFor="cloud-ftp-pass">Password</label>
          <input
            id="cloud-ftp-pass"
            type="password"
            autoComplete="off"
            value={draft.password}
            onChange={(e) => patch({ password: e.target.value })}
          />
          <label htmlFor="cloud-ftp-path">Base path</label>
          <input
            id="cloud-ftp-path"
            autoComplete="off"
            placeholder="/glint"
            value={draft.basePath}
            onChange={(e) => patch({ basePath: e.target.value })}
          />
          <label htmlFor="cloud-ftp-port">Port (optional)</label>
          <input
            id="cloud-ftp-port"
            inputMode="numeric"
            placeholder="21 or 990"
            value={draft.port}
            onChange={(e) => patch({ port: e.target.value })}
          />
          <label htmlFor="cloud-ftp-secure">
            <input
              id="cloud-ftp-secure"
              type="checkbox"
              checked={draft.secure}
              onChange={(e) => patch({ secure: e.target.checked })}
              style={{ marginRight: 8 }}
            />
            Use FTPS (recommended)
          </label>
          {!draft.secure ? (
            <p className="settings-hint" style={{ color: '#fbbf24' }}>
              Plain FTP sends credentials unencrypted.
            </p>
          ) : null}
        </>
      )}

      {draft.provider === 'drive' && (
        <>
          <label htmlFor="cloud-drive-token">Access token</label>
          <input
            id="cloud-drive-token"
            type="password"
            autoComplete="off"
            placeholder="Paste OAuth access token (v1)"
            value={draft.accessToken}
            onChange={(e) => patch({ accessToken: e.target.value })}
          />
          <label htmlFor="cloud-drive-folder">Folder ID (optional)</label>
          <input
            id="cloud-drive-folder"
            autoComplete="off"
            value={draft.folderId}
            onChange={(e) => patch({ folderId: e.target.value })}
          />
          <p className="settings-hint">
            Paste a short-lived Drive access token. Browser OAuth is deferred.
          </p>
        </>
      )}

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 4 }}>
        <button
          type="button"
          className="btn-primary"
          disabled={busy}
          onClick={() => void saveConfig()}
        >
          {busy ? 'Working…' : 'Save provider'}
        </button>
        <button
          type="button"
          className="btn-secondary"
          disabled={busy}
          onClick={() => void testConnection()}
        >
          Test connection
        </button>
      </div>
      {msg ? (
        <p className="settings-hint" style={{ marginTop: 8, color: 'var(--online)' }}>
          {msg}
        </p>
      ) : null}
      {err ? (
        <p className="settings-hint" style={{ marginTop: 8, color: '#fca5a5' }}>
          {err}
        </p>
      ) : null}
    </div>
  );
}
