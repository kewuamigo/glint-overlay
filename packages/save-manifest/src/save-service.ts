import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import type { SaveAPI, SaveLocation, SaveManifestEntry } from '@glint/plugin-sdk';
import { ManifestStore } from './manifest-db.js';
import { readSteamAppId } from './game-matcher.js';
import { resolvePlaceholders, type PlaceholderContext } from './placeholder-resolver.js';

const VALID_TAGS = new Set(['save', 'config', 'settings']);

type SaveTag = SaveLocation['tags'][number];

function detectSteamRoot(gameDir: string): string {
  const normalized = gameDir.replace(/\\/g, '/');
  const marker = '/steamapps/common/';
  const idx = normalized.toLowerCase().indexOf(marker);
  if (idx !== -1) {
    return gameDir.slice(0, idx);
  }
  return gameDir;
}

async function detectStoreUserId(steamRoot: string, appId: number | null): Promise<string> {
  if (appId === null) {
    return '';
  }

  const userdataDir = path.join(steamRoot, 'userdata');
  try {
    const users = await fs.readdir(userdataDir);
    for (const userId of users) {
      if (!/^\d+$/.test(userId)) continue;
      const candidate = path.join(userdataDir, userId, String(appId));
      try {
        await fs.access(candidate);
        return userId;
      } catch {
        // try next user
      }
    }
  } catch {
    // no userdata
  }

  return '';
}

async function expandPathTemplates(
  rawPath: string,
  ctx: PlaceholderContext,
): Promise<string[]> {
  if (!rawPath.includes('<storeUserId>')) {
    return [resolvePlaceholders(rawPath, ctx)];
  }

  const paths = new Set<string>();
  if (ctx.storeUserId) {
    paths.add(resolvePlaceholders(rawPath, ctx));
  }

  const parentTemplate = rawPath
    .slice(0, rawPath.indexOf('<storeUserId>'))
    .replace(/[/\\]+$/, '');
  const parent = resolvePlaceholders(parentTemplate, ctx);

  try {
    const entries = await fs.readdir(parent, { withFileTypes: true });
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      paths.add(resolvePlaceholders(rawPath, { ...ctx, storeUserId: entry.name }));
    }
  } catch {
    // parent missing
  }

  if (paths.size === 0) {
    paths.add(resolvePlaceholders(rawPath, ctx));
  }

  return [...paths];
}

function buildPlaceholderContext(
  gameDir: string,
  steamRoot: string,
  storeUserId: string,
  steamId: number | null,
): PlaceholderContext {
  const home = os.homedir();
  return {
    home,
    winLocalAppData: process.env.LOCALAPPDATA ?? path.join(home, 'AppData', 'Local'),
    winAppData: process.env.APPDATA ?? path.join(home, 'AppData', 'Roaming'),
    winDocuments: process.env.USERPROFILE
      ? path.join(process.env.USERPROFILE, 'Documents')
      : path.join(home, 'Documents'),
    storeGameDir: gameDir,
    storeUserDir: gameDir,
    storeUserId,
    root: steamRoot,
    ...(steamId !== null ? { steamId: String(steamId) } : {}),
  };
}

function normalizeTags(tags: string[] | undefined): SaveTag[] {
  if (!tags) {
    return ['save'];
  }
  const normalized = tags.filter((tag): tag is SaveTag => VALID_TAGS.has(tag));
  return normalized.length > 0 ? normalized : ['save'];
}

export class SaveManifestService implements SaveAPI {
  private gameDir = '';
  private exePath = '';
  private exeName = '';
  private store: ManifestStore | null = null;

  // `undefined` = not yet resolved; `null` = resolved but no Ludusavi match.
  // Using a distinct sentinel prevents re-running the expensive matchGame()
  // SQLite scan on every API call — it only runs once per game session.
  private manifestKey: string | null | undefined = undefined;
  private placeholderContext: PlaceholderContext | null = null;

  // Single in-flight promise so concurrent panel API calls share one lookup.
  private contextPromise: Promise<void> | null = null;

  constructor(opts?: { gameDir: string; exePath: string }) {
    if (opts) {
      this.applyGameContext(opts.gameDir, opts.exePath);
    }
  }

  async setGameContext(_pid: number, exePath: string): Promise<void> {
    this.applyGameContext(path.dirname(exePath), exePath);
    this.manifestKey = undefined;
    this.placeholderContext = null;
    this.contextPromise = null;
    await this.ensureContext();
  }

  private applyGameContext(gameDir: string, exePath: string): void {
    this.gameDir = gameDir;
    this.exePath = exePath;
    this.exeName = path.basename(exePath);
  }

  private async ensureStore(): Promise<ManifestStore> {
    if (!this.store) {
      this.store = await ManifestStore.open();
    }
    return this.store;
  }

  private async ensureContext(): Promise<void> {
    if (!this.gameDir) return;

    // Already resolved (even if the result was null = no match).
    if (this.manifestKey !== undefined) return;

    // Deduplicate concurrent calls — all await the same resolution.
    if (!this.contextPromise) {
      this.contextPromise = this.resolveContext();
    }
    return this.contextPromise;
  }

  private async resolveContext(): Promise<void> {
    const steamRoot = detectSteamRoot(this.gameDir);
    const steamId = readSteamAppId(this.gameDir);
    const storeUserId = await detectStoreUserId(steamRoot, steamId);
    this.placeholderContext = buildPlaceholderContext(
      this.gameDir,
      steamRoot,
      storeUserId,
      steamId,
    );

    const store = await this.ensureStore();
    // matchGame runs a synchronous SQLite LIKE full-table-scan — expensive.
    // We cache the result here so it never runs more than once per session.
    this.manifestKey = store.matchGame({
      gameDir: this.gameDir,
      exeName: this.exeName,
    });
  }

  async getGameDir(): Promise<string> {
    return this.gameDir;
  }

  async getManifestEntry(): Promise<SaveManifestEntry | null> {
    await this.ensureContext();
    if (!this.manifestKey) {
      return null;
    }
    const store = await this.ensureStore();
    return store.getGame(this.manifestKey);
  }

  async getSaveLocations(): Promise<SaveLocation[]> {
    const entry = await this.getManifestEntry();
    if (!entry || !this.placeholderContext) {
      return [];
    }

    const locations: SaveLocation[] = [];
    for (const [rawPath, meta] of Object.entries(entry.files)) {
      const resolvedPaths = await expandPathTemplates(rawPath, this.placeholderContext);
      const tags = normalizeTags(meta.tags);
      const filteredPaths = tags.includes('save')
        ? resolvedPaths.filter((candidate) => {
            const base = path.basename(candidate);
            return /^\d+$/.test(base);
          })
        : resolvedPaths;
      const pathsToUse = filteredPaths.length > 0 ? filteredPaths : resolvedPaths;

      for (const resolved of pathsToUse) {
        let exists = false;
        try {
          await fs.access(resolved);
          exists = true;
        } catch {
          exists = false;
        }
        locations.push({
          path: resolved,
          tags,
          exists,
        });
      }
    }

    return locations;
  }

  async findGame(query: string): Promise<SaveManifestEntry[]> {
    const store = await this.ensureStore();
    return store.searchGames(query);
  }

  async updateManifest(): Promise<{ updated: boolean; gameCount: number }> {
    this.store?.close();
    this.store = null;
    this.store = await ManifestStore.open({ forceNetwork: true });
    // Reset context so the next API call re-runs matchGame with the new DB.
    this.manifestKey = undefined;
    this.placeholderContext = null;
    this.contextPromise = null;
    await this.ensureContext();
    return {
      updated: true,
      gameCount: this.store.countGames(),
    };
  }
}
