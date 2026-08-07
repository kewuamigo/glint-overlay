import fs from 'node:fs';
import path from 'node:path';
import { parse } from 'yaml';
import { DatabaseSync } from 'node:sqlite';

interface RawManifestEntry {
  files?: Record<string, { tags?: string[] }>;
  steam?: { id?: number };
  notes?: string[];
  alias?: string;
}

function normalize(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, '');
}

export function buildDbFromYaml(yamlPath: string, dbPath: string): void {
  const raw = fs.readFileSync(yamlPath, 'utf8');
  const parsed = parse(raw) as Record<string, RawManifestEntry>;

  fs.mkdirSync(path.dirname(dbPath), { recursive: true });
  if (fs.existsSync(dbPath)) {
    fs.unlinkSync(dbPath);
  }

  const db = new DatabaseSync(dbPath);
  db.exec(`
    PRAGMA journal_mode = OFF;
    PRAGMA synchronous = OFF;
    CREATE TABLE games (
      title TEXT PRIMARY KEY,
      steam_id INTEGER,
      notes TEXT
    );
    CREATE INDEX idx_games_steam_id ON games(steam_id) WHERE steam_id IS NOT NULL;
    CREATE TABLE game_aliases (
      alias_title TEXT PRIMARY KEY,
      target_title TEXT NOT NULL
    );
    CREATE TABLE game_files (
      game_title TEXT NOT NULL,
      path_template TEXT NOT NULL,
      tags TEXT NOT NULL DEFAULT '["save"]'
    );
    CREATE INDEX idx_game_files_game ON game_files(game_title);
    CREATE TABLE game_lookup (
      game_title TEXT NOT NULL,
      norm TEXT NOT NULL,
      kind TEXT NOT NULL
    );
    CREATE INDEX idx_game_lookup_norm_kind ON game_lookup(norm, kind);
  `);

  const insertGame = db.prepare(
    'INSERT INTO games (title, steam_id, notes) VALUES (?, ?, ?)',
  );
  const insertAlias = db.prepare(
    'INSERT INTO game_aliases (alias_title, target_title) VALUES (?, ?)',
  );
  const insertFile = db.prepare(
    'INSERT INTO game_files (game_title, path_template, tags) VALUES (?, ?, ?)',
  );
  const insertLookup = db.prepare(
    'INSERT INTO game_lookup (game_title, norm, kind) VALUES (?, ?, ?)',
  );

  db.exec('BEGIN');
  try {
    for (const [title, entry] of Object.entries(parsed)) {
      const steamId = entry.steam?.id ?? null;
      const notes = entry.notes ? JSON.stringify(entry.notes) : null;
      const hasFiles = Object.keys(entry.files ?? {}).length > 0;

      if (entry.alias) {
        insertAlias.run(title, entry.alias);
        insertGame.run(title, steamId, notes);
        insertLookup.run(entry.alias, normalize(title), 'title');
        if (!hasFiles) {
          continue;
        }
      } else {
        insertGame.run(title, steamId, notes);
        insertLookup.run(title, normalize(title), 'title');
      }

      for (const [filePath, meta] of Object.entries(entry.files ?? {})) {
        const tags = Array.isArray(meta.tags) && meta.tags.length > 0
          ? meta.tags
          : ['save'];
        insertFile.run(title, filePath, JSON.stringify(tags));
      }
    }
    db.exec('COMMIT');
  } catch (err) {
    db.exec('ROLLBACK');
    db.close();
    throw err;
  }

  db.close();
}
