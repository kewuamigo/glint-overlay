import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { Worker } from 'node:worker_threads';
import { DatabaseSync } from 'node:sqlite';
import type { SaveManifestEntry } from '@glint/plugin-sdk';
import { readSteamAppId } from './game-matcher.js';

const MANIFEST_URL =
  'https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml';

const CACHE_MAX_AGE_MS = 24 * 60 * 60 * 1000;

export type LudusaviManifest = Record<string, SaveManifestEntry>;

interface ManifestMeta {
  downloadedAt: string;
}

function getCacheDir(): string {
  const appData = process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming');
  return path.join(appData, 'Glint', 'apps', 'save-manager', 'data');
}

function getLegacyCacheDir(): string {
  const appData = process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming');
  return path.join(appData, 'Glint', 'save-manifest');
}

/** One-shot move of Ludusavi cache from Glint/save-manifest → apps/save-manager/data. */
async function migrateLegacyCacheDir(): Promise<void> {
  const legacy = getLegacyCacheDir();
  const next = getCacheDir();
  const files = ['manifest.db', 'manifest.yaml', 'manifest.meta.json'] as const;

  let hasLegacy = false;
  for (const name of files) {
    try {
      await fs.access(path.join(legacy, name));
      hasLegacy = true;
      break;
    } catch {
      // missing
    }
  }
  if (!hasLegacy) return;

  await fs.mkdir(next, { recursive: true });
  for (const name of files) {
    const from = path.join(legacy, name);
    const to = path.join(next, name);
    try {
      await fs.access(from);
    } catch {
      continue;
    }
    try {
      await fs.access(to);
      continue; // keep newer location if already present
    } catch {
      await fs.rename(from, to).catch(async () => {
        await fs.copyFile(from, to);
      });
    }
  }
}

function getCachePaths(): {
  manifestPath: string;
  dbPath: string;
  metaPath: string;
} {
  const cacheDir = getCacheDir();
  return {
    manifestPath: path.join(cacheDir, 'manifest.yaml'),
    dbPath: path.join(cacheDir, 'manifest.db'),
    metaPath: path.join(cacheDir, 'manifest.meta.json'),
  };
}

function normalize(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, '');
}

async function readMetaIfFresh(): Promise<ManifestMeta | null> {
  const { metaPath } = getCachePaths();
  try {
    const metaRaw = await fs.readFile(metaPath, 'utf8');
    const meta = JSON.parse(metaRaw) as ManifestMeta;
    const age = Date.now() - new Date(meta.downloadedAt).getTime();
    if (age < CACHE_MAX_AGE_MS) {
      return meta;
    }
  } catch {
    // cache miss or stale
  }
  return null;
}

async function writeMeta(): Promise<void> {
  const { metaPath } = getCachePaths();
  await fs.mkdir(path.dirname(metaPath), { recursive: true });
  const meta: ManifestMeta = { downloadedAt: new Date().toISOString() };
  await fs.writeFile(metaPath, JSON.stringify(meta, null, 2), 'utf8');
}

async function fetchManifestYaml(): Promise<string> {
  const response = await fetch(MANIFEST_URL);
  if (!response.ok) {
    throw new Error(`Failed to fetch manifest: ${response.status} ${response.statusText}`);
  }
  return await response.text();
}

async function findFixturePath(): Promise<string> {
  if (process.env.GLINT_REPO_ROOT) {
    return path.join(process.env.GLINT_REPO_ROOT, 'data', 'save-manifest', 'fixture.yaml');
  }

  let dir = process.cwd();
  while (true) {
    const candidate = path.join(dir, 'data', 'save-manifest', 'fixture.yaml');
    try {
      await fs.access(candidate);
      return candidate;
    } catch {
      // continue walking up
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }

  throw new Error('Could not find data/save-manifest/fixture.yaml');
}

async function ensureYamlOnDisk(forceNetwork = false): Promise<string> {
  const { manifestPath } = getCachePaths();

  if (!forceNetwork) {
    const meta = await readMetaIfFresh();
    if (meta) {
      try {
        await fs.access(manifestPath);
        return manifestPath;
      } catch {
        // fall through
      }
    }
  }

  try {
    const content = await fetchManifestYaml();
    await fs.mkdir(path.dirname(manifestPath), { recursive: true });
    await fs.writeFile(manifestPath, content, 'utf8');
    await writeMeta();
    return manifestPath;
  } catch {
    return findFixturePath();
  }
}

/**
 * Rebuild the manifest database on a worker thread. Parsing the ~17MB YAML and
 * inserting ~50k games takes tens of seconds; doing it synchronously on the
 * Electron main thread freezes the whole overlay (input injection, global
 * shortcuts and IPC all stop) for the duration.
 */
function buildDbInWorker(yamlPath: string, dbPath: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./rebuild-worker.js', import.meta.url), {
      workerData: { yamlPath, dbPath },
    });
    worker.once('message', (msg: { ok: boolean; error?: string }) => {
      if (msg?.ok) {
        resolve();
      } else {
        reject(new Error(msg?.error ?? 'manifest db rebuild failed'));
      }
    });
    worker.once('error', reject);
    worker.once('exit', (code) => {
      if (code !== 0) {
        reject(new Error(`manifest rebuild worker exited with code ${code}`));
      }
    });
  });
}

let inflightOpen: Promise<ManifestStore> | null = null;

export class ManifestStore {
  private static active: ManifestStore | null = null;

  private constructor(private db: DatabaseSync) {}

  close(): void {
    this.db.close();
    if (ManifestStore.active === this) {
      ManifestStore.active = null;
    }
  }

  static async open(options?: { forceNetwork?: boolean }): Promise<ManifestStore> {
    if (options?.forceNetwork) {
      ManifestStore.active?.close();
      inflightOpen = null;
      const yamlPath = await ensureYamlOnDisk(true);
      await ManifestStore.rebuildDatabase(yamlPath);
      await writeMeta();
    } else if (inflightOpen) {
      return inflightOpen;
    }

    inflightOpen = ManifestStore.openOnce();
    try {
      return await inflightOpen;
    } catch (err) {
      inflightOpen = null;
      throw err;
    }
  }

  private static async openOnce(): Promise<ManifestStore> {
    if (ManifestStore.active) {
      return ManifestStore.active;
    }

    await migrateLegacyCacheDir();

    const { dbPath } = getCachePaths();
    const meta = await readMetaIfFresh();

    if (!meta) {
      const yamlPath = await ensureYamlOnDisk(false);
      await ManifestStore.rebuildDatabase(yamlPath);
      if (!(await readMetaIfFresh())) {
        await writeMeta();
      }
    } else {
      try {
        await fs.access(dbPath);
      } catch {
        const yamlPath = await ensureYamlOnDisk(false);
        await ManifestStore.rebuildDatabase(yamlPath);
      }
    }

    const db = new DatabaseSync(dbPath, { readOnly: true });
    const store = new ManifestStore(db);
    ManifestStore.active = store;
    return store;
  }

  private static async rebuildDatabase(yamlPath: string): Promise<void> {
    ManifestStore.active?.close();
    inflightOpen = null;

    const { dbPath } = getCachePaths();
    const tempPath = `${dbPath}.tmp`;
    await buildDbInWorker(yamlPath, tempPath);

    try {
      await fs.unlink(dbPath);
    } catch {
      // first build
    }
    await fs.rename(tempPath, dbPath);
  }

  private resolveCanonicalTitle(title: string): string {
    let current = title;
    for (let depth = 0; depth < 4; depth += 1) {
      const row = this.db
        .prepare('SELECT target_title FROM game_aliases WHERE alias_title = ?')
        .get(current) as { target_title: string } | undefined;
      if (!row) return current;
      current = row.target_title;
    }
    return current;
  }

  countGames(): number {
    const row = this.db
      .prepare('SELECT COUNT(*) AS count FROM games')
      .get() as { count: number };
    return row.count;
  }

  matchGame(opts: { gameDir: string; exeName: string }): string | null {
    const steamAppId = readSteamAppId(opts.gameDir);
    if (steamAppId !== null) {
      const bySteam = this.db
        .prepare('SELECT title FROM games WHERE steam_id = ?')
        .get(steamAppId) as { title: string } | undefined;
      if (bySteam) return bySteam.title;
    }

    const folderName = path.basename(opts.gameDir);
    const folderNorm = normalize(folderName);
    const byFolder = this.db
      .prepare(
        "SELECT game_title FROM game_lookup WHERE norm = ? AND kind = 'title' LIMIT 1",
      )
      .get(folderNorm) as { game_title: string } | undefined;
    if (byFolder) return byFolder.game_title;

    // Substring matching is only meaningful for reasonably long names on BOTH
    // sides. Without the length guards, titles that normalize to '' or one
    // character (e.g. "***", "B", "C") match every unknown game and produce a
    // garbage manifest entry with bogus save locations.
    const MIN_FUZZY_LEN = 5;
    const exeStem = path.basename(opts.exeName, path.extname(opts.exeName));
    const needles = [normalize(exeStem), folderNorm].filter(
      (n) => n.length >= MIN_FUZZY_LEN,
    );

    for (const needle of needles) {
      const fuzzy = this.db
        .prepare(
          `SELECT game_title FROM game_lookup
           WHERE kind = 'title' AND length(norm) >= ? AND (
             norm LIKE '%' || ? || '%' OR ? LIKE '%' || norm || '%'
           )
           LIMIT 1`,
        )
        .get(MIN_FUZZY_LEN, needle, needle) as { game_title: string } | undefined;
      if (fuzzy) return fuzzy.game_title;
    }

    return null;
  }

  getGame(title: string): SaveManifestEntry | null {
    const canonical = this.resolveCanonicalTitle(title);
    const game = this.db
      .prepare('SELECT title, steam_id, notes FROM games WHERE title = ?')
      .get(canonical) as { title: string; steam_id: number | null; notes: string | null } | undefined;
    if (!game) return null;

    const files = this.db
      .prepare(
        'SELECT path_template, tags FROM game_files WHERE game_title = ? ORDER BY rowid',
      )
      .all(canonical) as Array<{ path_template: string; tags: string }>;

    const entry: SaveManifestEntry = {
      title: canonical,
      files: {},
      ...(game.steam_id !== null ? { steamId: game.steam_id } : {}),
      ...(game.notes ? { notes: JSON.parse(game.notes) as string[] } : {}),
    };

    for (const file of files) {
      entry.files[file.path_template] = {
        tags: JSON.parse(file.tags) as string[],
      };
    }

    return entry;
  }

  searchGames(query: string, limit = 50): SaveManifestEntry[] {
    const needle = `%${query.toLowerCase()}%`;
    const rows = this.db
      .prepare(
        `SELECT title FROM games
         WHERE lower(title) LIKE ?
         ORDER BY title
         LIMIT ?`,
      )
      .all(needle, limit) as Array<{ title: string }>;

    return rows
      .map((row) => this.getGame(row.title))
      .filter((entry): entry is SaveManifestEntry => entry !== null);
  }

  toManifest(): LudusaviManifest {
    const rows = this.db
      .prepare('SELECT title FROM games ORDER BY title')
      .all() as Array<{ title: string }>;
    const manifest: LudusaviManifest = {};
    for (const row of rows) {
      const entry = this.getGame(row.title);
      if (entry) manifest[row.title] = entry;
    }
    return manifest;
  }
}

export function warmManifestCache(): void {
  void ManifestStore.open().catch(() => undefined);
}

export async function loadManifest(options?: { forceNetwork?: boolean }): Promise<LudusaviManifest> {
  const store = await ManifestStore.open(options);
  return store.toManifest();
}
