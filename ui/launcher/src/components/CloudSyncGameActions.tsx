import { useCallback, useEffect, useState } from 'react';
import {
  launcherInvoke,
  onSyncStatus,
  type CloudSyncStatusResponse,
  type GameSyncState,
  type ScannedGame,
  type SyncRevisionInfo,
} from '../launcher-bridge';

type Props = {
  game: ScannedGame;
};

function formatSyncTime(iso: string | null): string {
  if (!iso) return 'Never';
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

export function CloudSyncGameActions({ game }: Props) {
  const [state, setState] = useState<GameSyncState | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [revisions, setRevisions] = useState<SyncRevisionInfo[] | null>(null);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await launcherInvoke<CloudSyncStatusResponse>(
        'cloud.sync.getStatus',
        [game.id],
      );
      setEnabled(res.enabled);
      setConfigured(res.configured);
      setState(res.game ?? null);
    } catch {
      // sync not ready
    }
  }, [game.id]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    return onSyncStatus((payload) => {
      if (payload.gameId === game.id) {
        setState(payload.status);
      }
    });
  }, [game.id]);

  const run = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true);
    setErr(null);
    try {
      const result = await fn();
      if (
        result &&
        typeof result === 'object' &&
        'ok' in result &&
        (result as { ok: boolean }).ok === false
      ) {
        setErr(
          String(
            (result as { reason?: string; error?: string }).reason ??
              (result as { error?: string }).error ??
              'Sync failed',
          ),
        );
        await refresh();
        return;
      }
      await refresh();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadRevisions = async () => {
    setBusy(true);
    setErr(null);
    try {
      const list = await launcherInvoke<SyncRevisionInfo[]>(
        'cloud.sync.listRevisions',
        [game.id],
      );
      setRevisions(list);
      setPendingDelete(null);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const confirmDelete = async (revisionId: string) => {
    setBusy(true);
    setErr(null);
    try {
      await launcherInvoke('cloud.sync.deleteRevision', [game.id, revisionId]);
      setPendingDelete(null);
      const list = await launcherInvoke<SyncRevisionInfo[]>(
        'cloud.sync.listRevisions',
        [game.id],
      );
      setRevisions(list);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!enabled && !configured) return null;

  const conflict = state?.conflictPending === true;
  const statusLine = !configured
    ? 'Configure a provider in Settings'
    : !enabled
      ? 'Cloud sync disabled'
      : conflict
        ? 'Conflict pending — remote is newer'
        : state?.lastStatus === 'syncing'
          ? 'Syncing…'
          : state?.lastStatus === 'error'
            ? `Last error: ${state.lastError ?? 'unknown'}`
            : `Last sync: ${formatSyncTime(state?.lastSuccessfulSync ?? null)}`;

  return (
    <div className="cloud-sync-actions">
      <div className="cloud-sync-status-line tabular-nums">{statusLine}</div>

      {conflict ? (
        <div className="cloud-sync-conflict">
          <span>Remote revision is newer than this device.</span>
          <div className="cloud-sync-btns">
            <button
              type="button"
              className="btn-secondary"
              disabled={busy}
              onClick={() =>
                void run(() =>
                  launcherInvoke('cloud.sync.now', [game, { overwrite: true }]),
                )
              }
            >
              Overwrite remote
            </button>
            <button
              type="button"
              className="btn-secondary"
              disabled={busy}
              onClick={() =>
                void run(() =>
                  launcherInvoke('cloud.sync.now', [game, { keepRemote: true }]),
                )
              }
            >
              Keep remote
            </button>
          </div>
        </div>
      ) : null}

      <div className="cloud-sync-btns">
        <button
          type="button"
          className="btn-secondary"
          disabled={busy || !enabled || !configured}
          onClick={() => void run(() => launcherInvoke('cloud.sync.now', [game]))}
        >
          {busy ? 'Working…' : 'Sync now'}
        </button>
        <button
          type="button"
          className="btn-secondary"
          disabled={busy || !configured}
          onClick={() => {
            if (revisions) {
              setRevisions(null);
              setPendingDelete(null);
              return;
            }
            void loadRevisions();
          }}
        >
          {revisions ? 'Hide revisions' : 'Revisions'}
        </button>
      </div>

      {revisions ? (
        <ul className="cloud-sync-revisions">
          {revisions.length === 0 ? (
            <li className="cloud-sync-revision-empty">No remote revisions</li>
          ) : (
            revisions
              .slice()
              .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
              .slice(0, 8)
              .map((r) => (
                <li key={r.revisionId} className="cloud-sync-revision-row">
                  <span className="cloud-sync-revision-meta tabular-nums">
                    {formatSyncTime(r.createdAt)}
                    <span className="cloud-sync-revision-id">
                      {r.revisionId.slice(0, 8)}
                    </span>
                  </span>
                  <div className="cloud-sync-revision-actions">
                    {pendingDelete === r.revisionId ? (
                      <>
                        <button
                          type="button"
                          className="btn-ghost cloud-sync-danger"
                          disabled={busy}
                          onClick={() => void confirmDelete(r.revisionId)}
                        >
                          Confirm delete
                        </button>
                        <button
                          type="button"
                          className="btn-ghost"
                          disabled={busy}
                          onClick={() => setPendingDelete(null)}
                        >
                          Cancel
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          type="button"
                          className="btn-ghost"
                          disabled={busy}
                          onClick={() =>
                            void run(() =>
                              launcherInvoke('cloud.sync.restore', [
                                game,
                                r.revisionId,
                              ]),
                            )
                          }
                        >
                          Restore
                        </button>
                        <button
                          type="button"
                          className="btn-ghost cloud-sync-danger"
                          disabled={busy}
                          onClick={() => setPendingDelete(r.revisionId)}
                        >
                          Delete
                        </button>
                      </>
                    )}
                  </div>
                </li>
              ))
          )}
        </ul>
      ) : null}

      {err ? <div className="cloud-sync-msg err">{err}</div> : null}
    </div>
  );
}
