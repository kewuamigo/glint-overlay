import { useCallback, useEffect, useState } from 'react';
import {
  launcherInvoke,
  onSyncStatus,
  type CloudSyncStatusResponse,
  type GameSyncState,
} from '../launcher-bridge';

export type CloudSyncPlayPhase = 'idle' | 'syncing';

function isSyncingState(state: GameSyncState | null | undefined, phase?: string): boolean {
  if (phase === 'start' || phase === 'upload') return true;
  return state?.lastStatus === 'syncing';
}

/** Maps cloud sync status for one game to Play-button Syncing state. */
export function useCloudSyncPlayState(gameId: string | undefined): {
  syncing: boolean;
  state: GameSyncState | null;
} {
  const [state, setState] = useState<GameSyncState | null>(null);
  const [syncing, setSyncing] = useState(false);

  const refresh = useCallback(async () => {
    if (!gameId) {
      setState(null);
      setSyncing(false);
      return;
    }
    try {
      const res = await launcherInvoke<CloudSyncStatusResponse>(
        'cloud.sync.getStatus',
        [gameId],
      );
      const g = res.game ?? null;
      setState(g);
      setSyncing(isSyncingState(g));
    } catch {
      setState(null);
      setSyncing(false);
    }
  }, [gameId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!gameId) return;
    return onSyncStatus((payload) => {
      if (payload.gameId !== gameId) return;
      setState(payload.status);
      setSyncing(isSyncingState(payload.status, payload.phase));
    });
  }, [gameId]);

  return { syncing, state };
}
