import fs from 'node:fs';
import path from 'node:path';
import { APP_DATA_DIR_NAME } from './product.js';

export type SessionSnapshotEntry = {
  pid: number;
  name: string;
  totalSeconds: number;
};

export type SessionSnapshot = { games: SessionSnapshotEntry[] };

export type SessionSnapshotGame = {
  id: string;
  name: string;
  pid?: number;
  running: boolean;
};

/** `%APPDATA%/Glint/session.json` sidecar path (db-free for tests). */
export function sessionSnapshotPath(appData = process.env.APPDATA ?? ''): string {
  return path.join(appData, APP_DATA_DIR_NAME, 'session.json');
}

/** Running tracked games as pid-keyed sidecar entries; others are dropped. */
export function buildSessionSnapshot(
  games: SessionSnapshotGame[],
  totals: Record<string, number>,
): SessionSnapshot {
  const entries: SessionSnapshotEntry[] = [];
  for (const game of games) {
    if (!game.running || !game.pid) continue;
    entries.push({
      pid: game.pid,
      name: game.name,
      totalSeconds: totals[game.id] ?? 0,
    });
  }
  return { games: entries };
}

/** Atomic-ish sidecar write (temp + rename); best-effort, never throws. */
export function writeSessionSnapshot(
  snapshot: SessionSnapshot,
  appData = process.env.APPDATA ?? '',
): void {
  try {
    const file = sessionSnapshotPath(appData);
    const tmp = `${file}.tmp`;
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(tmp, JSON.stringify(snapshot));
    fs.renameSync(tmp, file);
  } catch {
    // Display-only sidecar; a failed write must not break the scan loop.
  }
}
