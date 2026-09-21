import { useState } from 'react';
import { useOta } from '../hooks/useOta';

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function UpdateBanner() {
  const ota = useOta();
  const [applyError, setApplyError] = useState<string | null>(null);
  const status = ota.status;
  const available = status?.available;
  if (!status?.enabled || !available || status.dismissed) return null;
  if (status.state !== 'available' && status.state !== 'downloading' && status.state !== 'applying' && status.state !== 'error') {
    return null;
  }

  const portable = status.layout === 'portable';
  const missingSetup = !portable && !available.setupAsset;
  const downloading = status.state === 'downloading' || status.state === 'applying';
  const pct =
    status.download && status.download.total && status.download.total > 0
      ? Math.min(100, Math.round((status.download.received / status.download.total) * 100))
      : null;

  const primaryLabel = portable
    ? 'Open release'
    : missingSetup
      ? 'Open release'
      : downloading
        ? status.state === 'applying'
          ? 'Starting installer…'
          : 'Downloading…'
        : 'Update now';

  const onPrimary = async () => {
    setApplyError(null);
    try {
      if (portable || missingSetup) {
        await ota.openRelease();
      } else {
        await ota.apply();
      }
    } catch (err) {
      setApplyError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="ota-banner" role="status">
      <div className="ota-banner-copy">
        <strong>Glint {available.version} is available</strong>
        <p>
          {portable
            ? 'This copy is portable. Download the new zip from GitHub Releases — Setup cannot overwrite this folder.'
            : missingSetup
              ? 'This release has no Setup installer. Open the GitHub Release to download it.'
              : available.notes ??
                'Close games first if overlay files may be locked. The installer will relaunch Glint when it finishes.'}
        </p>
        {downloading && status.download ? (
          <p className="ota-banner-progress">
            {pct != null
              ? `${pct}% · ${formatBytes(status.download.received)} / ${formatBytes(status.download.total ?? 0)}`
              : `${formatBytes(status.download.received)} downloaded`}
          </p>
        ) : null}
        {applyError || (status.state === 'error' && status.lastError) ? (
          <p className="ota-banner-error">{applyError ?? status.lastError}</p>
        ) : null}
      </div>
      {downloading && pct != null ? (
        <div className="progress-bar ota-progress" aria-hidden>
          <div className="progress-fill" style={{ width: `${pct}%` }} />
        </div>
      ) : null}
      <div className="ota-banner-actions">
        <button
          type="button"
          className="btn-primary"
          disabled={downloading || ota.busy}
          onClick={() => void onPrimary()}
        >
          {primaryLabel}
        </button>
        <button
          type="button"
          className="btn-secondary"
          disabled={downloading}
          onClick={() => void ota.dismiss()}
        >
          Later
        </button>
        {!portable && !missingSetup ? (
          <button
            type="button"
            className="btn-ghost"
            disabled={downloading}
            onClick={() => void ota.openRelease()}
          >
            View release
          </button>
        ) : null}
      </div>
    </div>
  );
}
