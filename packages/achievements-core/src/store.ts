import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';

export type AchievementSource =
  | 'steam-official'
  | 'gse'
  | 'epic-official'
  | 'epic-emu';

export type TrackedGame = {
  id: string;
  name: string;
  platform: string;
  exe_path: string | null;
  process_name: string | null;
  source: AchievementSource;
};

export type AchievementRow = {
  game_id: string;
  achievement_id: string;
  title: string;
  description: string | null;
  icon_unlocked: string | null;
  unlocked: number;
  unlocked_at: number | null;
  progress: number | null;
};

function dataDir(): string {
  return path.join(
    process.env.APPDATA ?? '',
    'Glint',
    'apps',
    'achievements',
    'data',
  );
}

export function achievementsDbPath(): string {
  return path.join(dataDir(), 'app.db');
}

let db: DatabaseSync | null = null;

export function getAchievementsDb(): DatabaseSync {
  if (db) return db;
  fs.mkdirSync(dataDir(), { recursive: true });
  db = new DatabaseSync(achievementsDbPath());
  db.exec('PRAGMA journal_mode = WAL');
  db.exec('PRAGMA busy_timeout = 5000');
  db.exec(`
    CREATE TABLE IF NOT EXISTS sync_state (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS tracked_games (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      platform TEXT NOT NULL,
      exe_path TEXT,
      process_name TEXT,
      source TEXT NOT NULL,
      updated_at INTEGER NOT NULL,
      deleted_at INTEGER,
      rev INTEGER NOT NULL DEFAULT 1
    );
    CREATE TABLE IF NOT EXISTS achievement_defs (
      game_id TEXT NOT NULL,
      achievement_id TEXT NOT NULL,
      title TEXT NOT NULL,
      description TEXT,
      icon_locked TEXT,
      icon_unlocked TEXT,
      updated_at INTEGER NOT NULL,
      rev INTEGER NOT NULL DEFAULT 1,
      PRIMARY KEY (game_id, achievement_id)
    );
    CREATE TABLE IF NOT EXISTS achievement_state (
      game_id TEXT NOT NULL,
      achievement_id TEXT NOT NULL,
      unlocked INTEGER NOT NULL DEFAULT 0,
      unlocked_at INTEGER,
      progress REAL,
      updated_at INTEGER NOT NULL,
      deleted_at INTEGER,
      rev INTEGER NOT NULL DEFAULT 1,
      PRIMARY KEY (game_id, achievement_id)
    );
  `);
  const device = db
    .prepare(`SELECT value FROM sync_state WHERE key = 'device_id'`)
    .get() as { value: string } | undefined;
  if (!device) {
    db.prepare(`INSERT INTO sync_state (key, value) VALUES ('device_id', ?)`).run(
      randomUUID(),
    );
  }
  return db;
}

export function upsertTrackedGame(game: {
  id: string;
  name: string;
  platform: string;
  source: AchievementSource;
  exe_path?: string | null;
  process_name?: string | null;
}): void {
  const now = Date.now();
  getAchievementsDb()
    .prepare(
      `
      INSERT INTO tracked_games
        (id, name, platform, exe_path, process_name, source, updated_at, deleted_at, rev)
      VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1)
      ON CONFLICT(id) DO UPDATE SET
        name = excluded.name,
        platform = excluded.platform,
        exe_path = COALESCE(excluded.exe_path, tracked_games.exe_path),
        process_name = COALESCE(excluded.process_name, tracked_games.process_name),
        source = excluded.source,
        updated_at = excluded.updated_at,
        rev = tracked_games.rev + 1,
        deleted_at = NULL
    `,
    )
    .run(
      game.id,
      game.name,
      game.platform,
      game.exe_path ?? null,
      game.process_name ?? null,
      game.source,
      now,
    );
}

/** Returns true if this is a newly recorded unlock. */
export function upsertAchievementDef(input: {
  gameId: string;
  achievementId: string;
  title: string;
  description?: string | null;
  iconLocked?: string | null;
  iconUnlocked?: string | null;
}): void {
  const now = Date.now();
  getAchievementsDb()
    .prepare(
      `
      INSERT INTO achievement_defs
        (game_id, achievement_id, title, description, icon_locked, icon_unlocked, updated_at, rev)
      VALUES (?, ?, ?, ?, ?, ?, ?, 1)
      ON CONFLICT(game_id, achievement_id) DO UPDATE SET
        title = excluded.title,
        description = COALESCE(excluded.description, achievement_defs.description),
        icon_locked = COALESCE(excluded.icon_locked, achievement_defs.icon_locked),
        icon_unlocked = COALESCE(excluded.icon_unlocked, achievement_defs.icon_unlocked),
        updated_at = excluded.updated_at,
        rev = achievement_defs.rev + 1
    `,
    )
    .run(
      input.gameId,
      input.achievementId,
      input.title,
      input.description ?? null,
      input.iconLocked ?? null,
      input.iconUnlocked ?? null,
      now,
    );
}

/** Returns true if this is a newly recorded unlock. */
export function recordUnlock(input: {
  gameId: string;
  achievementId: string;
  title: string;
  description?: string | null;
  iconUnlocked?: string | null;
  unlockedAt?: number | null;
}): boolean {
  const now = Date.now();
  upsertAchievementDef({
    gameId: input.gameId,
    achievementId: input.achievementId,
    title: input.title,
    description: input.description,
    iconUnlocked: input.iconUnlocked,
  });

  const database = getAchievementsDb();
  const prev = database
    .prepare(
      `SELECT unlocked FROM achievement_state
       WHERE game_id = ? AND achievement_id = ? AND deleted_at IS NULL`,
    )
    .get(input.gameId, input.achievementId) as { unlocked: number } | undefined;

  const wasUnlocked = prev?.unlocked === 1;
  database
    .prepare(
      `
      INSERT INTO achievement_state
        (game_id, achievement_id, unlocked, unlocked_at, progress, updated_at, deleted_at, rev)
      VALUES (?, ?, 1, ?, NULL, ?, NULL, 1)
      ON CONFLICT(game_id, achievement_id) DO UPDATE SET
        unlocked = 1,
        unlocked_at = COALESCE(achievement_state.unlocked_at, excluded.unlocked_at),
        updated_at = excluded.updated_at,
        rev = achievement_state.rev + 1,
        deleted_at = NULL
    `,
    )
    .run(input.gameId, input.achievementId, input.unlockedAt ?? now, now);

  return !wasUnlocked;
}

export function listTrackedGames(): TrackedGame[] {
  return getAchievementsDb()
    .prepare(
      `SELECT id, name, platform, exe_path, process_name, source
       FROM tracked_games WHERE deleted_at IS NULL ORDER BY updated_at DESC`,
    )
    .all() as TrackedGame[];
}

export function listAchievementsForGame(gameId: string): AchievementRow[] {
  return getAchievementsDb()
    .prepare(
      `
      SELECT
        d.game_id,
        d.achievement_id,
        d.title,
        d.description,
        d.icon_unlocked,
        COALESCE(s.unlocked, 0) AS unlocked,
        s.unlocked_at,
        s.progress
      FROM achievement_defs d
      LEFT JOIN achievement_state s
        ON s.game_id = d.game_id AND s.achievement_id = d.achievement_id
      WHERE d.game_id = ?
      ORDER BY COALESCE(s.unlocked, 0) DESC, d.title
    `,
    )
    .all(gameId) as AchievementRow[];
}
