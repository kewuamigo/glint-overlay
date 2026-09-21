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
      logo TEXT,
      icon_mime TEXT,
      grid_mime TEXT,
      hero_mime TEXT,
      logo_mime TEXT,
      icon_manual INTEGER NOT NULL DEFAULT 0,
      grid_manual INTEGER NOT NULL DEFAULT 0,
      hero_manual INTEGER NOT NULL DEFAULT 0,
      logo_manual INTEGER NOT NULL DEFAULT 0,
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

  const coverCols = database
    .prepare(`PRAGMA table_info(covers)`)
    .all() as Array<{ name: string }>;
  const have = new Set(coverCols.map((c) => c.name));
  const addCol = (name: string, def: string) => {
    if (!have.has(name)) {
      database.exec(`ALTER TABLE covers ADD COLUMN ${name} ${def}`);
      have.add(name);
    }
  };
  addCol('logo', 'TEXT');
  for (const slot of ['icon', 'grid', 'hero', 'logo'] as const) {
    addCol(`${slot}_mime`, 'TEXT');
    addCol(`${slot}_manual`, 'INTEGER NOT NULL DEFAULT 0');
  }
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

export type ArtSlot = 'icon' | 'grid' | 'hero' | 'logo';

export type CoverRow = {
  icon: string | null;
  grid: string | null;
  hero: string | null;
  logo: string | null;
  icon_mime: string | null;
  grid_mime: string | null;
  hero_mime: string | null;
  logo_mime: string | null;
  icon_manual: number;
  grid_manual: number;
  hero_manual: number;
  logo_manual: number;
};

export function getCover(gameId: string): CoverRow | null {
  const row = getDb()
    .prepare(
      `SELECT icon, grid, hero, logo,
              icon_mime, grid_mime, hero_mime, logo_mime,
              icon_manual, grid_manual, hero_manual, logo_manual
       FROM covers WHERE game_id = ?`,
    )
    .get(gameId) as CoverRow | undefined;
  return row ?? null;
}

/** Auto-resolve write: never overwrite slots marked manual. */
export function setCover(
  gameId: string,
  art: {
    icon?: string;
    grid?: string;
    hero?: string;
    logo?: string;
    iconMime?: string;
    gridMime?: string;
    heroMime?: string;
    logoMime?: string;
  },
): void {
  const existing = getCover(gameId);
  const pick = (
    slot: ArtSlot,
    next: string | undefined,
    nextMime: string | undefined,
  ): { url: string | null; mime: string | null } => {
    const manualKey = `${slot}_manual` as keyof CoverRow;
    const mimeKey = `${slot}_mime` as keyof CoverRow;
    if (existing && Number(existing[manualKey]) === 1) {
      return {
        url: (existing[slot] as string | null) ?? null,
        mime: (existing[mimeKey] as string | null) ?? null,
      };
    }
    return {
      url: next !== undefined ? next : ((existing?.[slot] as string | null) ?? null),
      mime:
        nextMime !== undefined
          ? nextMime
          : ((existing?.[mimeKey] as string | null) ?? null),
    };
  };

  const icon = pick('icon', art.icon, art.iconMime);
  const grid = pick('grid', art.grid, art.gridMime);
  const hero = pick('hero', art.hero, art.heroMime);
  const logo = pick('logo', art.logo, art.logoMime);

  getDb()
    .prepare(
      `
      INSERT INTO covers (
        game_id, icon, grid, hero, logo,
        icon_mime, grid_mime, hero_mime, logo_mime,
        icon_manual, grid_manual, hero_manual, logo_manual,
        updated_at
      )
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(game_id) DO UPDATE SET
        icon = excluded.icon,
        grid = excluded.grid,
        hero = excluded.hero,
        logo = excluded.logo,
        icon_mime = excluded.icon_mime,
        grid_mime = excluded.grid_mime,
        hero_mime = excluded.hero_mime,
        logo_mime = excluded.logo_mime,
        updated_at = excluded.updated_at
    `,
    )
    .run(
      gameId,
      icon.url,
      grid.url,
      hero.url,
      logo.url,
      icon.mime,
      grid.mime,
      hero.mime,
      logo.mime,
      existing?.icon_manual ?? 0,
      existing?.grid_manual ?? 0,
      existing?.hero_manual ?? 0,
      existing?.logo_manual ?? 0,
      Date.now(),
    );
}

export function setCoverManual(
  gameId: string,
  slot: ArtSlot,
  url: string,
  mime: string | null,
): void {
  const existing = getCover(gameId);
  const next = {
    icon: existing?.icon ?? null as string | null,
    grid: existing?.grid ?? null as string | null,
    hero: existing?.hero ?? null as string | null,
    logo: existing?.logo ?? null as string | null,
    icon_mime: existing?.icon_mime ?? null as string | null,
    grid_mime: existing?.grid_mime ?? null as string | null,
    hero_mime: existing?.hero_mime ?? null as string | null,
    logo_mime: existing?.logo_mime ?? null as string | null,
    icon_manual: existing?.icon_manual ?? 0,
    grid_manual: existing?.grid_manual ?? 0,
    hero_manual: existing?.hero_manual ?? 0,
    logo_manual: existing?.logo_manual ?? 0,
  };
  if (slot === 'icon') {
    next.icon = url;
    next.icon_mime = mime;
    next.icon_manual = 1;
  } else if (slot === 'grid') {
    next.grid = url;
    next.grid_mime = mime;
    next.grid_manual = 1;
  } else if (slot === 'hero') {
    next.hero = url;
    next.hero_mime = mime;
    next.hero_manual = 1;
  } else {
    next.logo = url;
    next.logo_mime = mime;
    next.logo_manual = 1;
  }

  getDb()
    .prepare(
      `
      INSERT INTO covers (
        game_id, icon, grid, hero, logo,
        icon_mime, grid_mime, hero_mime, logo_mime,
        icon_manual, grid_manual, hero_manual, logo_manual,
        updated_at
      )
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(game_id) DO UPDATE SET
        icon = excluded.icon,
        grid = excluded.grid,
        hero = excluded.hero,
        logo = excluded.logo,
        icon_mime = excluded.icon_mime,
        grid_mime = excluded.grid_mime,
        hero_mime = excluded.hero_mime,
        logo_mime = excluded.logo_mime,
        icon_manual = excluded.icon_manual,
        grid_manual = excluded.grid_manual,
        hero_manual = excluded.hero_manual,
        logo_manual = excluded.logo_manual,
        updated_at = excluded.updated_at
    `,
    )
    .run(
      gameId,
      next.icon,
      next.grid,
      next.hero,
      next.logo,
      next.icon_mime,
      next.grid_mime,
      next.hero_mime,
      next.logo_mime,
      next.icon_manual,
      next.grid_manual,
      next.hero_manual,
      next.logo_manual,
      Date.now(),
    );
}

/** Drop manual lock for a slot so the next resolve can refill it. */
export function clearCoverManual(gameId: string, slot: ArtSlot): void {
  const existing = getCover(gameId);
  if (!existing) return;
  getDb()
    .prepare(
      `UPDATE covers SET ${slot}_manual = 0, ${slot} = NULL, ${slot}_mime = NULL, updated_at = ? WHERE game_id = ?`,
    )
    .run(Date.now(), gameId);
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
