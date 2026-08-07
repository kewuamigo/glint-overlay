import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import Database from 'better-sqlite3';
import { APP_DATA_DIR_NAME } from './product.js';

const SECRET_KEYS = new Set(['steamApiKey', 'steamGridDbApiKey']);

let db: Database.Database | null = null;

function dataDir(): string {
  return path.join(process.env.APPDATA ?? '', APP_DATA_DIR_NAME);
}

export function dbPath(): string {
  return path.join(dataDir(), 'launcher.db');
}

export function getDb(): Database.Database {
  if (db) return db;

  const dir = dataDir();
  fs.mkdirSync(dir, { recursive: true });
  db = new Database(dbPath());
  db.pragma('journal_mode = WAL');
  db.pragma('foreign_keys = ON');
  migrateSchema(db);
  migrateFromJson(db);
  seedSyncState(db);
  return db;
}

function migrateSchema(database: Database.Database): void {
  database.exec(`
    CREATE TABLE IF NOT EXISTS settings (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL,
      updated_at INTEGER NOT NULL,
      deleted_at INTEGER,
      rev INTEGER NOT NULL DEFAULT 1,
      syncable INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS custom_games (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL,
      executable TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL,
      deleted_at INTEGER,
      rev INTEGER NOT NULL DEFAULT 1
    );

    CREATE TABLE IF NOT EXISTS covers (
      game_id TEXT PRIMARY KEY,
      icon TEXT,
      grid TEXT,
      hero TEXT,
      updated_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS playtime (
      game_id TEXT PRIMARY KEY,
      name TEXT,
      total_seconds INTEGER NOT NULL DEFAULT 0,
      last_played_at INTEGER,
      updated_at INTEGER NOT NULL,
      deleted_at INTEGER,
      rev INTEGER NOT NULL DEFAULT 1
    );

    CREATE TABLE IF NOT EXISTS sync_state (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
  `);
}

function seedSyncState(database: Database.Database): void {
  const row = database
    .prepare(`SELECT value FROM sync_state WHERE key = 'device_id'`)
    .get() as { value: string } | undefined;
  if (!row) {
    database
      .prepare(`INSERT INTO sync_state (key, value) VALUES ('device_id', ?)`)
      .run(randomUUID());
  }
}

function migrateFromJson(database: Database.Database): void {
  const flag = database
    .prepare(`SELECT value FROM sync_state WHERE key = 'json_migrated'`)
    .get() as { value: string } | undefined;
  if (flag?.value === '1') return;

  const now = Date.now();
  const dir = dataDir();
  let migrationOk = true;

  const settingsFile = path.join(dir, 'launcher-settings.json');
  if (fs.existsSync(settingsFile)) {
    try {
      const data = JSON.parse(fs.readFileSync(settingsFile, 'utf8')) as Record<
        string,
        unknown
      >;
      const upsert = database.prepare(`
        INSERT INTO settings (key, value, updated_at, deleted_at, rev, syncable)
        VALUES (?, ?, ?, NULL, 1, ?)
        ON CONFLICT(key) DO UPDATE SET
          value = excluded.value,
          updated_at = excluded.updated_at,
          rev = settings.rev + 1,
          syncable = excluded.syncable,
          deleted_at = NULL
      `);
      for (const key of ['steamApiKey', 'steamGridDbApiKey'] as const) {
        if (typeof data[key] === 'string') {
          upsert.run(key, data[key], now, SECRET_KEYS.has(key) ? 0 : 1);
        }
      }
    } catch {
      migrationOk = false;
    }
  }

  const gamesFile = path.join(dir, 'launcher-games.json');
  if (fs.existsSync(gamesFile)) {
    try {
      const data = JSON.parse(fs.readFileSync(gamesFile, 'utf8')) as {
        games?: Array<{ name?: string; executable?: string }>;
      };
      const insert = database.prepare(`
        INSERT OR IGNORE INTO custom_games
          (id, name, executable, created_at, updated_at, deleted_at, rev)
        VALUES (?, ?, ?, ?, ?, NULL, 1)
      `);
      for (const g of data.games ?? []) {
        const name = typeof g.name === 'string' ? g.name.trim() : '';
        const exe = typeof g.executable === 'string' ? g.executable.trim() : '';
        if (!name || !exe) continue;
        insert.run(`custom:${exe}`, name, exe, now, now);
      }
    } catch {
      migrationOk = false;
    }
  }

  const coversFile = path.join(dir, 'covers-cache.json');
  if (fs.existsSync(coversFile)) {
    try {
      const cache = JSON.parse(fs.readFileSync(coversFile, 'utf8')) as Record<
        string,
        { icon?: string; grid?: string; hero?: string; at?: number }
      >;
      const insert = database.prepare(`
        INSERT OR REPLACE INTO covers (game_id, icon, grid, hero, updated_at)
        VALUES (?, ?, ?, ?, ?)
      `);
      for (const [gameId, art] of Object.entries(cache)) {
        if (!art?.icon && !art?.grid && !art?.hero) continue;
        insert.run(
          gameId,
          art.icon ?? null,
          art.grid ?? null,
          art.hero ?? null,
          typeof art.at === 'number' ? art.at : now,
        );
      }
    } catch {
      migrationOk = false;
    }
  }

  if (!migrationOk) return;

  database
    .prepare(
      `INSERT INTO sync_state (key, value) VALUES ('json_migrated', '1')
       ON CONFLICT(key) DO UPDATE SET value = '1'`,
    )
    .run();
}

export function getSetting(key: string): string | null {
  const row = getDb()
    .prepare(
      `SELECT value FROM settings WHERE key = ? AND deleted_at IS NULL`,
    )
    .get(key) as { value: string } | undefined;
  return row?.value ?? null;
}

export function setSetting(key: string, value: string): void {
  const now = Date.now();
  const syncable = SECRET_KEYS.has(key) ? 0 : 1;
  getDb()
    .prepare(
      `
      INSERT INTO settings (key, value, updated_at, deleted_at, rev, syncable)
      VALUES (?, ?, ?, NULL, 1, ?)
      ON CONFLICT(key) DO UPDATE SET
        value = excluded.value,
        updated_at = excluded.updated_at,
        rev = settings.rev + 1,
        syncable = excluded.syncable,
        deleted_at = NULL
    `,
    )
    .run(key, value, now, syncable);
}

export function listCustomGames(): Array<{
  id: string;
  name: string;
  executable: string;
}> {
  return getDb()
    .prepare(
      `SELECT id, name, executable FROM custom_games WHERE deleted_at IS NULL ORDER BY created_at`,
    )
    .all() as Array<{ id: string; name: string; executable: string }>;
}

export function insertCustomGame(
  name: string,
  executable: string,
): { id: string; name: string; executable: string } {
  const now = Date.now();
  const id = `custom:${executable}`;
  const existing = getDb()
    .prepare(
      `SELECT id FROM custom_games
       WHERE lower(executable) = lower(?) AND deleted_at IS NULL`,
    )
    .get(executable) as { id: string } | undefined;
  if (existing) {
    throw new Error('game with this executable already exists');
  }
  getDb()
    .prepare(
      `
      INSERT INTO custom_games (id, name, executable, created_at, updated_at, deleted_at, rev)
      VALUES (?, ?, ?, ?, ?, NULL, 1)
      ON CONFLICT(id) DO UPDATE SET
        name = excluded.name,
        executable = excluded.executable,
        updated_at = excluded.updated_at,
        rev = custom_games.rev + 1,
        deleted_at = NULL
    `,
    )
    .run(id, name, executable, now, now);
  return { id, name, executable };
}

export function softDeleteAllCustomGames(): void {
  const now = Date.now();
  getDb()
    .prepare(
      `UPDATE custom_games SET deleted_at = ?, updated_at = ?, rev = rev + 1
       WHERE deleted_at IS NULL`,
    )
    .run(now, now);
}

export function getCover(gameId: string): {
  icon: string | null;
  grid: string | null;
  hero: string | null;
} | null {
  const row = getDb()
    .prepare(`SELECT icon, grid, hero FROM covers WHERE game_id = ?`)
    .get(gameId) as
    | { icon: string | null; grid: string | null; hero: string | null }
    | undefined;
  return row ?? null;
}

export function setCover(
  gameId: string,
  art: { icon?: string; grid?: string; hero?: string },
): void {
  getDb()
    .prepare(
      `
      INSERT INTO covers (game_id, icon, grid, hero, updated_at)
      VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(game_id) DO UPDATE SET
        icon = excluded.icon,
        grid = excluded.grid,
        hero = excluded.hero,
        updated_at = excluded.updated_at
    `,
    )
    .run(
      gameId,
      art.icon ?? null,
      art.grid ?? null,
      art.hero ?? null,
      Date.now(),
    );
}

export function getAllPlaytimeSeconds(): Record<string, number> {
  const rows = getDb()
    .prepare(
      `SELECT game_id, total_seconds FROM playtime WHERE deleted_at IS NULL`,
    )
    .all() as Array<{ game_id: string; total_seconds: number }>;
  const out: Record<string, number> = {};
  for (const row of rows) out[row.game_id] = row.total_seconds;
  return out;
}

/** Accumulate elapsed seconds for a running game (capped per tick). */
export function accumulatePlaytime(
  gameId: string,
  name: string,
  deltaSeconds: number,
): void {
  if (deltaSeconds <= 0) return;
  const now = Date.now();
  getDb()
    .prepare(
      `
      INSERT INTO playtime
        (game_id, name, total_seconds, last_played_at, updated_at, deleted_at, rev)
      VALUES (?, ?, ?, ?, ?, NULL, 1)
      ON CONFLICT(game_id) DO UPDATE SET
        name = excluded.name,
        total_seconds = playtime.total_seconds + excluded.total_seconds,
        last_played_at = excluded.last_played_at,
        updated_at = excluded.updated_at,
        rev = playtime.rev + 1,
        deleted_at = NULL
    `,
    )
    .run(gameId, name, deltaSeconds, now, now);
}
