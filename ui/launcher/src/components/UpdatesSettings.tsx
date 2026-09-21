import { useState } from 'react';
import { useOta } from '../hooks/useOta';

function formatWhen(iso: string | null): string {
  if (!iso) return 'Never';
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return iso;
  return new Date(t).toLocaleString();
}

export function UpdatesSettings() {
  const ota = useOta();
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const status = ota.status;

  if (!status) {
    return (
      <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
        <label>Updates</label>
        <p className="settings-hint">Loading version…</p>
      </div>
    );
  }

  if (!status.enabled) {
    return (
      <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
        <label>Updates</label>
        <p className="settings-hint">
          Current version <code>{status.currentVersion}</code>. Automatic updates are
          disabled by <code>GLINT_OTA=0</code>.
        </p>
      </div>
    );
  }

  const resultLine = (() => {
    if (status.state === 'checking') return 'Checking GitHub Releases…';
    if (status.state === 'downloading') return 'Downloading installer…';
    if (status.state === 'applying') return 'Starting installer…';
    if (status.available) {
      return `Update ${status.available.version} is available.`;
    }
    if (status.state === 'error' && status.lastError) return status.lastError;
    if (status.state === 'up-to-date') return 'You are up to date.';
    return 'No check yet.';
  })();

  const checkNow = async () => {
    setErr(null);
    setMsg(null);
    try {
      const next = await ota.check();
      if (next.state === 'error' && next.lastError) {
        setErr(next.lastError);
        return;
      }
      setMsg('Checked just now.');
      setTimeout(() => setMsg(null), 2000);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  const apply = async () => {
    setErr(null);
    try {
      if (status.layout === 'portable' || !status.available?.setupAsset) {
        await ota.openRelease();
        return;
      }
      await ota.apply();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="settings-section glass" style={{ padding: 20, marginBottom: 16 }}>
      <label>Updates</label>
      <p className="settings-hint" style={{ marginTop: 0 }}>
        Current version <code>{status.currentVersion}</code>
        {status.layout === 'portable' ? ' · portable layout' : ''}.
      </p>
      <label htmlFor="ota-auto-check" style={{ marginTop: 12 }}>
        <input
          id="ota-auto-check"
          type="checkbox"
          checked={status.autoCheck}
          onChange={(e) => void ota.setAutoCheck(e.target.checked)}
          style={{ marginRight: 8 }}
        />
        Check for updates automatically
      </label>
      <p className="settings-hint">
        At most once per 24 hours after startup. Manual check always runs now.
      </p>
      <p className="settings-hint">Last check: {formatWhen(status.lastCheckAt)}</p>
      <p className="settings-hint">{resultLine}</p>
      {status.layout === 'portable' && status.available ? (
        <p className="settings-hint">
          Portable copies cannot run Setup in-place. Open the GitHub Release to
          download the zip or installer.
        </p>
      ) : null}
      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 12 }}>
        <button
          type="button"
          className="btn-primary"
          disabled={ota.busy}
          onClick={() => void checkNow()}
        >
          {ota.busy && status.state === 'checking' ? 'Checking…' : 'Check for updates'}
        </button>
        {status.available ? (
          <button
            type="button"
            className="btn-secondary"
            disabled={ota.busy || status.state === 'downloading' || status.state === 'applying'}
            onClick={() => void apply()}
          >
            {status.layout === 'portable' || !status.available.setupAsset
              ? 'Open release'
              : 'Update now'}
          </button>
        ) : null}
        {status.available ? (
          <button
            type="button"
            className="btn-ghost"
            onClick={() => void ota.openRelease()}
          >
            View release
          </button>
        ) : null}
      </div>
      {msg ? <p className="settings-hint" style={{ marginTop: 8 }}>{msg}</p> : null}
      {err ? (
        <p className="settings-hint ota-banner-error" style={{ marginTop: 8 }}>
          {err}
        </p>
      ) : null}
    </div>
  );
}
