import { accumulatePlaytime, getAllPlaytimeSeconds, getDb } from './db.js';
import { buildSessionSnapshot, writeSessionSnapshot } from './session-snapshot.js';

/** Last tick timestamp per running game_id. */
const lastTick = new Map<string, number>();

const MAX_DELTA_SEC = 30;

/**
 * Call on each games.scan while games may be running.
 * Accumulates wall-clock seconds since the previous tick for each running game,
 * then refreshes the pid-keyed session.json sidecar.
 */
export function tickPlaytime(
  games: Array<{ id: string; name: string; pid?: number; running: boolean }>,
): void {
  getDb();
  const now = Date.now();
  const runningIds = new Set<string>();

  for (const game of games) {
    if (!game.running) continue;
    runningIds.add(game.id);
    const prev = lastTick.get(game.id);
    lastTick.set(game.id, now);
    if (prev == null) continue;
    const delta = Math.min(MAX_DELTA_SEC, Math.floor((now - prev) / 1000));
    if (delta > 0) {
      accumulatePlaytime(game.id, game.name, delta);
    }
  }

  for (const id of [...lastTick.keys()]) {
    if (!runningIds.has(id)) lastTick.delete(id);
  }

  writeSessionSnapshot(buildSessionSnapshot(games, getAllPlaytimeSeconds()));
}
