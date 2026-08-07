import fs from 'node:fs';
import path from 'node:path';
import type { LudusaviManifest } from './manifest-db.js';

export function readSteamAppId(gameDir: string): number | null {
  const appIdPath = path.join(gameDir, 'steam_appid.txt');
  try {
    const raw = fs.readFileSync(appIdPath, 'utf8').trim();
    const id = Number.parseInt(raw, 10);
    return Number.isFinite(id) ? id : null;
  } catch {
    return null;
  }
}

function normalize(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, '');
}

function fuzzyMatch(a: string, b: string): boolean {
  const na = normalize(a);
  const nb = normalize(b);
  if (!na || !nb) {
    return false;
  }
  return na.includes(nb) || nb.includes(na);
}

export function matchGame(
  manifest: LudusaviManifest,
  opts: { gameDir: string; exeName: string },
): string | null {
  const steamAppId = readSteamAppId(opts.gameDir);
  if (steamAppId !== null) {
    for (const [key, entry] of Object.entries(manifest)) {
      if (entry.steamId === steamAppId) {
        return key;
      }
    }
  }

  const folderName = path.basename(opts.gameDir);
  for (const key of Object.keys(manifest)) {
    if (normalize(key) === normalize(folderName)) {
      return key;
    }
  }

  const exeStem = path.basename(opts.exeName, path.extname(opts.exeName));
  for (const key of Object.keys(manifest)) {
    if (fuzzyMatch(key, exeStem) || fuzzyMatch(key, folderName)) {
      return key;
    }
  }

  return null;
}
