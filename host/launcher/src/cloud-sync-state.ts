import fs from 'node:fs';
import path from 'node:path';
import { APP_DATA_DIR_NAME } from './product.js';

export type GameSyncStatus =
  | 'idle'
  | 'syncing'
  | 'ok'
  | 'error'
  | 'conflict'
  | 'skipped';

export type GameSyncState = {
  lastSuccessfulSync: string | null;
  conflictPending: boolean;
  lastError: string | null;
  lastStatus: GameSyncStatus;
};

export type CloudSyncStateFile = {
  games: Record<string, GameSyncState>;
};

const DEFAULT_GAME: GameSyncState = {
  lastSuccessfulSync: null,
  conflictPending: false,
  lastError: null,
  lastStatus: 'idle',
};

function statePath(): string {
  return path.join(
    process.env.APPDATA ?? '',
    APP_DATA_DIR_NAME,
    'cloud-sync-state.json',
  );
}

export function loadCloudSyncState(): CloudSyncStateFile {
  const file = statePath();
  try {
    if (!fs.existsSync(file)) return { games: {} };
    const raw = JSON.parse(fs.readFileSync(file, 'utf8')) as CloudSyncStateFile;
    if (!raw || typeof raw !== 'object' || typeof raw.games !== 'object') {
      return { games: {} };
    }
    return { games: raw.games ?? {} };
  } catch {
    return { games: {} };
  }
}

export function saveCloudSyncState(state: CloudSyncStateFile): void {
  const file = statePath();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(state, null, 2), 'utf8');
}

export function getGameSyncState(gameId: string): GameSyncState {
  const all = loadCloudSyncState();
  return { ...DEFAULT_GAME, ...(all.games[gameId] ?? {}) };
}

export function patchGameSyncState(
  gameId: string,
  patch: Partial<GameSyncState>,
): GameSyncState {
  const all = loadCloudSyncState();
  const next: GameSyncState = {
    ...DEFAULT_GAME,
    ...(all.games[gameId] ?? {}),
    ...patch,
  };
  all.games[gameId] = next;
  saveCloudSyncState(all);
  return next;
}
