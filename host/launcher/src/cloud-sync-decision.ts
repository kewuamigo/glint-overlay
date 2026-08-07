/** Pure conflict / upload-policy helpers (no Electron, unit-testable). */

export type SyncUploadMode = 'upload' | 'skip-conflict' | 'keep-remote';

export type ConflictCursor = {
  /** ISO timestamp of latest remote revision, or null if none */
  remoteCreatedAt: string | null;
  /** ISO timestamp of this device's lastSuccessfulSync, or null */
  lastSuccessfulSync: string | null;
};

/** Remote tip is newer than the device cursor (D8). */
export function remoteIsNewerThanCursor(c: ConflictCursor): boolean {
  if (!c.remoteCreatedAt) return false;
  if (!c.lastSuccessfulSync) return true;
  const remote = Date.parse(c.remoteCreatedAt);
  const local = Date.parse(c.lastSuccessfulSync);
  if (Number.isNaN(remote)) return false;
  if (Number.isNaN(local)) return true;
  return remote > local;
}

/**
 * Decide whether an upload may proceed.
 * - Unattended exit: never overwrite when remote is newer → skip-conflict
 * - Manual Sync Now: overwrite=true forces upload; keepRemote clears without upload
 */
export function decideUpload(opts: {
  remoteNewer: boolean;
  unattended: boolean;
  overwrite?: boolean;
  keepRemote?: boolean;
}): SyncUploadMode {
  if (opts.keepRemote) return 'keep-remote';
  if (!opts.remoteNewer) return 'upload';
  if (opts.overwrite) return 'upload';
  if (opts.unattended) return 'skip-conflict';
  // Manual sync without overwrite: surface conflict, do not upload
  return 'skip-conflict';
}

export function pickLatestRevision<T extends { createdAt: string }>(
  revisions: T[],
): T | null {
  if (revisions.length === 0) return null;
  let best = revisions[0]!;
  let bestMs = Date.parse(best.createdAt);
  for (let i = 1; i < revisions.length; i++) {
    const r = revisions[i]!;
    const ms = Date.parse(r.createdAt);
    if (!Number.isNaN(ms) && (Number.isNaN(bestMs) || ms > bestMs)) {
      best = r;
      bestMs = ms;
    }
  }
  return best;
}

/**
 * Normalize packed achievements.json: either a row array or
 * `{ gameId, achievements: [...] }` (legacy pack shape).
 */
export function unwrapAchievementRows(raw: unknown): unknown[] | null {
  if (Array.isArray(raw)) return raw;
  if (
    raw &&
    typeof raw === 'object' &&
    Array.isArray((raw as { achievements?: unknown }).achievements)
  ) {
    return (raw as { achievements: unknown[] }).achievements;
  }
  return null;
}
