const ID_RE = /^[a-zA-Z0-9._-]{1,128}$/;

/**
 * Storage key for ALL adapters (selfhost, ftp, drive).
 * Output always matches `[a-zA-Z0-9._-]{1,128}` (self-host server path rule).
 *
 * Callers MUST pack with `sanitizeGameId(gameId)` as `manifest.gameId` so the
 * revision tar matches remote URL/path keys. Adapters sanitize the method
 * argument before I/O and reject when the manifest diverges.
 */
export function sanitizeGameId(gameId: string): string {
  if (ID_RE.test(gameId)) return gameId;
  const cleaned = gameId.replace(/[^a-zA-Z0-9._-]/g, '_').slice(0, 128);
  return cleaned.length > 0 ? cleaned : 'game';
}

/** Sanitize caller id; when packing, require manifest.gameId === sanitized form. */
export function resolveGameId(gameId: string, manifestGameId?: string): string {
  const id = sanitizeGameId(gameId);
  if (manifestGameId !== undefined && manifestGameId !== id) {
    throw new Error(
      `Revision gameId mismatch: expected ${id}, got ${manifestGameId}. ` +
        `Pack with sanitizeGameId(gameId).`,
    );
  }
  return id;
}

export function assertRevisionId(revisionId: string): string {
  if (!ID_RE.test(revisionId)) {
    throw new Error(`Invalid revisionId: ${revisionId}`);
  }
  return revisionId;
}
