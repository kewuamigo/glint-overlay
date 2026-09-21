import { useState } from 'react';
import type { PrepareState, ScannedGame } from '../launcher-bridge';

export function PrepareProgress({ forceGse }: { forceGse?: boolean }) {
  return (
    <div className="prepare-progress" role="status" aria-live="polite">
      <p className="prepare-progress-label">
        {forceGse
          ? 'Installing GSE… first download can take a minute'
          : 'Creating achievement integration…'}
      </p>
      <div className="prepare-progress-bar" aria-hidden>
        <span className="prepare-progress-fill" />
      </div>
    </div>
  );
}

export function isMissingSteamAppIdError(error: string | undefined): boolean {
  return /appid/i.test(error ?? '');
}

type Props = {
  prepare: PrepareState | undefined;
  game: ScannedGame;
  errorClassName: string;
  onRetry: (game: ScannedGame, steamAppId?: string, forceGse?: boolean) => void;
};

export function PrepareFailedBanner({
  prepare,
  game,
  errorClassName,
  onRetry,
}: Props) {
  const [appId, setAppId] = useState('');
  const missingAppId = isMissingSteamAppIdError(prepare?.error);
  const trimmed = appId.trim();
  const canInstall = /^\d+$/.test(trimmed);

  return (
    <>
      <span className={errorClassName}>
        {prepare?.error ?? 'Achievement integration failed.'}
      </span>
      {missingAppId ? (
        <div className="prepare-appid-row">
          <input
            type="text"
            inputMode="numeric"
            pattern="[0-9]*"
            className="prepare-appid-input"
            placeholder="Steam AppID"
            value={appId}
            onChange={(e) => setAppId(e.target.value.replace(/\D/g, ''))}
            aria-label="Steam AppID"
          />
          <button
            type="button"
            className="btn-secondary"
            disabled={!canInstall}
            onClick={() => onRetry(game, trimmed, true)}
          >
            Install GSE
          </button>
        </div>
      ) : (
        <button
          type="button"
          className="btn-secondary"
          onClick={() => onRetry(game)}
        >
          Retry
        </button>
      )}
    </>
  );
}
