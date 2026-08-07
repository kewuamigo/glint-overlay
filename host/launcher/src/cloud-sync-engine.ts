import fsPromises from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  listAchievementsForGame,
  recordUnlock,
  resolveLibraryExePath,
  upsertAchievementDef,
  type AchievementRow,
} from '@glint/achievements-core';
import {
  packRevision,
  sanitizeGameId,
  unpackRevision,
  type SyncRevisionInfo,
} from '@glint/cloud-sync';
import { SaveManifestService } from '@glint/save-manifest';
import type { ScannedGame } from './custom-games.js';
import {
  getActiveStorageProvider,
  isCloudSyncReady,
  loadCloudStorageConfig,
} from './cloud-storage-config.js';
import {
  decideUpload,
  pickLatestRevision,
  remoteIsNewerThanCursor,
  unwrapAchievementRows,
} from './cloud-sync-decision.js';
import {
  getGameSyncState,
  loadCloudSyncState,
  patchGameSyncState,
  type GameSyncState,
} from './cloud-sync-state.js';
import { rustCli } from './rust-cli.js';

export type SyncGameRef = {
  id: string;
  name: string;
  exe: string;
  install_path?: string;
};

export type SyncNowOptions = {
  overwrite?: boolean;
  keepRemote?: boolean;
  /** Exit-triggered sync (D8: never silent overwrite). */
  unattended?: boolean;
};

export type SyncResult = {
  ok: boolean;
  skipped?: boolean;
  conflictPending?: boolean;
  reason?: string;
  revision?: SyncRevisionInfo;
  gameState: GameSyncState;
};

type StatusListener = (payload: {
  gameId: string;
  status: GameSyncState;
  phase?: string;
}) => void;

const tracked = new Map<number, SyncGameRef>();
const inFlight = new Map<string, Promise<SyncResult>>();
let statusListener: StatusListener | null = null;

export function setCloudSyncStatusListener(listener: StatusListener | null): void {
  statusListener = listener;
}

function emit(gameId: string, status: GameSyncState, phase?: string): void {
  statusListener?.({ gameId, status, phase });
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function isPidAlive(pid: number): boolean {
  try {
    const procs = rustCli<{ processes: { pid: number; name: string }[] }>(
      'processes',
    );
    return (procs.processes ?? []).some((p) => p.pid === pid);
  } catch {
    return false;
  }
}

async function waitForPidExit(pid: number): Promise<void> {
  while (isPidAlive(pid)) {
    await sleep(2000);
  }
}

/** Register a launched/attached game PID; on exit enqueue unattended sync. */
export function trackGameProcess(pid: number, game: SyncGameRef): void {
  if (!pid || tracked.has(pid)) return;
  tracked.set(pid, game);
  void waitForPidExit(pid).then(() => {
    tracked.delete(pid);
    if (!isCloudSyncReady()) return;
    void runSync(game, { unattended: true });
  });
}

export function resolveTrackedGameFromPid(
  pid: number,
  games: ScannedGame[],
): SyncGameRef | null {
  try {
    const procs = rustCli<{ processes: { pid: number; name: string }[] }>(
      'processes',
    );
    const proc = (procs.processes ?? []).find((p) => p.pid === pid);
    if (!proc) return null;
    const name = proc.name.toLowerCase();
    const hit = games.find(
      (g) => path.basename(g.exe).toLowerCase() === name,
    );
    if (!hit) return null;
    return {
      id: hit.id,
      name: hit.name,
      exe: hit.exe,
      install_path: hit.install_path,
    };
  } catch {
    return null;
  }
}

async function maxMtimeMs(root: string): Promise<number> {
  let max = 0;
  async function walk(p: string): Promise<void> {
    let st;
    try {
      st = await fsPromises.stat(p);
    } catch {
      return;
    }
    if (st.isFile()) {
      max = Math.max(max, st.mtimeMs);
      return;
    }
    if (!st.isDirectory()) return;
    max = Math.max(max, st.mtimeMs);
    let entries;
    try {
      entries = await fsPromises.readdir(p);
    } catch {
      return;
    }
    for (const name of entries) {
      await walk(path.join(p, name));
    }
  }
  await walk(root);
  return max;
}

async function resolveSavePaths(game: SyncGameRef): Promise<string[]> {
  const exePath = resolveLibraryExePath(game.exe, game.install_path ?? '');
  if (!exePath) return [];
  const gameDir =
    game.install_path && game.install_path.length > 0
      ? game.install_path
      : path.dirname(exePath);
  const svc = new SaveManifestService({ gameDir, exePath });
  const locations = await svc.getSaveLocations();
  return locations.filter((l) => l.exists).map((l) => l.path);
}

async function localIsDirty(
  game: SyncGameRef,
  savePaths: string[],
  lastSuccessfulSync: string | null,
): Promise<boolean> {
  if (!lastSuccessfulSync) {
    if (savePaths.length > 0) return true;
    return listAchievementsForGame(game.id).length > 0;
  }
  const cursor = Date.parse(lastSuccessfulSync);
  if (Number.isNaN(cursor)) return true;
  for (const p of savePaths) {
    if ((await maxMtimeMs(p)) > cursor) return true;
  }
  const ach = listAchievementsForGame(game.id);
  for (const row of ach) {
    if (row.unlocked_at != null && row.unlocked_at > cursor) return true;
  }
  return false;
}

function achievementsSnapshot(gameId: string): AchievementRow[] {
  return listAchievementsForGame(gameId);
}

function applyAchievementsSnapshot(
  gameId: string,
  raw: unknown,
): void {
  const rows = unwrapAchievementRows(raw);
  if (!rows) return;
  for (const item of rows) {
    if (!item || typeof item !== 'object') continue;
    const row = item as Partial<AchievementRow>;
    const achievementId = String(row.achievement_id ?? '');
    if (!achievementId) continue;
    const title = String(row.title ?? achievementId);
    upsertAchievementDef({
      gameId,
      achievementId,
      title,
      description: row.description ?? null,
      iconUnlocked: row.icon_unlocked ?? null,
    });
    if (Number(row.unlocked) === 1) {
      recordUnlock({
        gameId,
        achievementId,
        title,
        description: row.description ?? null,
        iconUnlocked: row.icon_unlocked ?? null,
        unlockedAt: row.unlocked_at ?? null,
      });
    }
  }
}

async function packAndUpload(
  game: SyncGameRef,
  provider: NonNullable<ReturnType<typeof getActiveStorageProvider>>,
): Promise<SyncRevisionInfo> {
  const gameId = sanitizeGameId(game.id);
  const savePaths = await resolveSavePaths(game);
  const achievements = achievementsSnapshot(game.id);
  const workRoot = await fsPromises.mkdtemp(
    path.join(os.tmpdir(), 'go-cloud-sync-'),
  );
  try {
    const { revisionDir, manifest } = await packRevision(workRoot, {
      gameId,
      saveLocations: savePaths,
      achievements: achievements.length > 0 ? achievements : undefined,
    });
    void manifest;
    return await provider.putRevision(gameId, revisionDir);
  } finally {
    await fsPromises.rm(workRoot, { recursive: true, force: true }).catch(() => {});
  }
}

export async function runSync(
  game: SyncGameRef,
  opts: SyncNowOptions = {},
): Promise<SyncResult> {
  const gameId = game.id;
  const existing = inFlight.get(gameId);
  if (existing) return existing;

  const job = (async (): Promise<SyncResult> => {
    if (!isCloudSyncReady()) {
      const gameState = getGameSyncState(gameId);
      return {
        ok: false,
        skipped: true,
        reason: 'Cloud sync disabled or backend not configured',
        gameState,
      };
    }

    const provider = getActiveStorageProvider();
    if (!provider) {
      const gameState = patchGameSyncState(gameId, {
        lastStatus: 'error',
        lastError: 'No storage provider',
      });
      return { ok: false, reason: 'No storage provider', gameState };
    }

    const unattended = opts.unattended === true;
    let gameState = patchGameSyncState(gameId, {
      lastStatus: 'syncing',
      lastError: null,
    });
    emit(gameId, gameState, 'start');

    try {
      const revisions = await provider.listRevisions(sanitizeGameId(game.id));
      const latest = pickLatestRevision(revisions);

      if (opts.keepRemote) {
        gameState = patchGameSyncState(gameId, {
          conflictPending: false,
          lastStatus: 'ok',
          lastError: null,
          ...(latest?.createdAt
            ? { lastSuccessfulSync: latest.createdAt }
            : {}),
        });
        emit(gameId, gameState, 'keep-remote');
        return { ok: true, skipped: true, reason: 'keep-remote', gameState };
      }
      const cursor = getGameSyncState(gameId).lastSuccessfulSync;
      const remoteNewer = remoteIsNewerThanCursor({
        remoteCreatedAt: latest?.createdAt ?? null,
        lastSuccessfulSync: cursor,
      });

      const decision = decideUpload({
        remoteNewer,
        unattended,
        overwrite: opts.overwrite,
        keepRemote: opts.keepRemote,
      });

      if (decision === 'skip-conflict') {
        gameState = patchGameSyncState(gameId, {
          conflictPending: true,
          lastStatus: 'conflict',
          lastError: unattended
            ? 'Remote revision newer than lastSuccessfulSync; skipped unattended upload'
            : 'Conflict pending — pass overwrite:true or keepRemote:true',
        });
        emit(gameId, gameState, 'conflict');
        return {
          ok: false,
          skipped: true,
          conflictPending: true,
          reason: gameState.lastError ?? 'conflict',
          gameState,
        };
      }

      emit(gameId, gameState, 'upload');
      const revision = await packAndUpload(game, provider);
      gameState = patchGameSyncState(gameId, {
        lastSuccessfulSync: revision.createdAt,
        conflictPending: false,
        lastStatus: 'ok',
        lastError: null,
      });
      emit(gameId, gameState, 'ok');
      return { ok: true, revision, gameState };
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      gameState = patchGameSyncState(gameId, {
        lastStatus: 'error',
        lastError: message,
      });
      emit(gameId, gameState, 'error');
      return { ok: false, reason: message, gameState };
    }
  })();

  inFlight.set(gameId, job);
  try {
    return await job;
  } finally {
    inFlight.delete(gameId);
  }
}

/** Catch-up before launch when local is dirty and no conflict pending. */
export async function catchUpBeforeLaunch(game: SyncGameRef): Promise<SyncResult | null> {
  if (!isCloudSyncReady()) return null;
  const state = getGameSyncState(game.id);
  if (state.conflictPending) return null;
  const savePaths = await resolveSavePaths(game);
  const dirty = await localIsDirty(game, savePaths, state.lastSuccessfulSync);
  if (!dirty) return null;
  return runSync(game, { unattended: false });
}

export function getCloudSyncStatus(gameId?: string): {
  enabled: boolean;
  configured: boolean;
  games: Record<string, GameSyncState>;
  game?: GameSyncState;
} {
  const { enabled, config } = loadCloudStorageConfig();
  const { games } = loadCloudSyncState();
  return {
    enabled,
    configured: config != null,
    games,
    ...(gameId ? { game: getGameSyncState(gameId) } : {}),
  };
}

export async function listCloudRevisions(
  gameId: string,
): Promise<SyncRevisionInfo[]> {
  if (!getActiveStorageProvider() && !loadCloudStorageConfig().config) {
    throw new Error('No cloud storage provider configured');
  }
  const provider = getActiveStorageProvider();
  if (!provider) throw new Error('No cloud storage provider configured');
  return provider.listRevisions(sanitizeGameId(gameId));
}

export async function deleteCloudRevision(
  gameId: string,
  revisionId: string,
): Promise<{ ok: true }> {
  const provider = getActiveStorageProvider();
  if (!provider) throw new Error('No cloud storage provider configured');
  const sid = sanitizeGameId(gameId);
  const rid = String(revisionId ?? '').trim();
  if (!rid) throw new Error('revisionId required');
  const existing = await provider.listRevisions(sid);
  if (!existing.some((r) => r.revisionId === rid)) {
    throw new Error(`Revision not found: ${rid}`);
  }
  await provider.deleteRevision(sid, rid);
  return { ok: true };
}

export async function restoreCloudRevision(
  game: SyncGameRef,
  revisionId?: string,
): Promise<{
  ok: boolean;
  revisionId: string;
  restoredSaves: Array<{ name: string; destPath: string }>;
  gameState: GameSyncState;
}> {
  const provider = getActiveStorageProvider();
  if (!provider) throw new Error('No cloud storage provider configured');

  const sid = sanitizeGameId(game.id);
  let revId: string | undefined = revisionId;
  if (!revId) {
    const list = await provider.listRevisions(sid);
    const latest = pickLatestRevision(list);
    if (!latest) throw new Error('No remote revision exists');
    revId = latest.revisionId;
  }
  if (!revId) throw new Error('No remote revision exists');

  const workRoot = await fsPromises.mkdtemp(
    path.join(os.tmpdir(), 'go-cloud-restore-'),
  );
  try {
    const revisionDir = await provider.getRevision(sid, revId, workRoot);
    const unpacked = await unpackRevision(revisionDir);
    applyAchievementsSnapshot(game.id, unpacked.achievements);
    const gameState = patchGameSyncState(game.id, {
      lastSuccessfulSync: unpacked.manifest.createdAt,
      conflictPending: false,
      lastStatus: 'ok',
      lastError: null,
    });
    emit(game.id, gameState, 'restored');
    return {
      ok: true,
      revisionId: revId,
      restoredSaves: unpacked.restoredSaves,
      gameState,
    };
  } finally {
    await fsPromises.rm(workRoot, { recursive: true, force: true }).catch(() => {});
  }
}

export function toSyncGameRef(raw: Partial<SyncGameRef> & { id?: string }): SyncGameRef {
  const id = String(raw.id ?? '');
  if (!id) throw new Error('gameId required');
  return {
    id,
    name: String(raw.name ?? id),
    exe: String(raw.exe ?? ''),
    install_path: raw.install_path ? String(raw.install_path) : '',
  };
}
