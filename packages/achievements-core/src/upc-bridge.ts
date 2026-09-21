import fs from 'node:fs';
import path from 'node:path';
import { detectScanDirs, resolveSteamAppId } from './detect.js';

const UPC_DLL_SCAN_BYTES = 2 * 1024 * 1024;
const UPC_LOADERS = [
  'upc_r2_loader64.dll',
  'upc_r2_loader.dll',
  'upc_r2_loader64.dll.bak',
  'upc_r2_loader.dll.bak',
];

export function mapUpcInId(inId: string, schemaNames: string[]): string | null {
  const raw = String(inId ?? '').trim();
  if (!raw || schemaNames.length === 0) return null;
  if (schemaNames.includes(raw)) return raw;
  if (!/^\d+$/.test(raw)) return null;
  const n = Number(raw);
  if (n >= 1 && n <= schemaNames.length) return schemaNames[n - 1];
  if (n >= 0 && n < schemaNames.length) return schemaNames[n];
  return null;
}

export function readSchemaNames(gameDir: string): string[] {
  const file = path.join(gameDir, 'steam_settings', 'achievements.json');
  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) return [];
  const names: string[] = [];
  for (const row of raw) {
    const name = row && typeof row === 'object' ? (row as { name?: unknown }).name : null;
    if (typeof name === 'string' && name) names.push(name);
  }
  return names;
}

export function mergeGseUnlock(
  filePath: string,
  steamName: string,
  earnedTimeSec: number,
): void {
  let map: Record<string, { earned: boolean; earned_time: number }> = {};
  try {
    const raw = JSON.parse(fs.readFileSync(filePath, 'utf8'));
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      map = raw as typeof map;
    }
  } catch {
    /* missing or junk — start empty */
  }
  map[steamName] = { earned: true, earned_time: earnedTimeSec };
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(map, null, 2)}\n`);
}

export function gseUnlockPath(appId: string, appData = process.env.APPDATA ?? ''): string {
  return path.join(appData, 'GSE Saves', appId, 'achievements.json');
}

export function schemaDirForExe(exePath: string): string {
  for (const dir of detectScanDirs(exePath)) {
    if (fs.existsSync(path.join(dir, 'steam_settings', 'achievements.json'))) {
      return dir;
    }
  }
  return path.dirname(exePath);
}

export function enableUpcAchievementsIni(iniPath: string): void {
  let text: string;
  try {
    text = fs.readFileSync(iniPath, 'utf8');
  } catch {
    return;
  }
  if (/^\s*Achievements\s*=\s*1\s*$/im.test(text)) return;
  if (/^\s*Achievements\s*=/im.test(text)) {
    text = text.replace(/^\s*Achievements\s*=\s*.*$/im, 'Achievements = 1');
  } else if (/^\[Settings\]/im.test(text)) {
    text = text.replace(/^\[Settings\][^\S\r\n]*/im, '[Settings]\nAchievements = 1');
  } else {
    text = `[Settings]\nAchievements = 1\n${text}`;
  }
  fs.writeFileSync(iniPath, text);
}

export function writeUpcAchievementsJson(
  destDir: string,
  rows: { name: string; displayName?: string; description?: string }[],
): void {
  const file = path.join(destDir, 'achievements.json');
  let existing: Record<string, { earned?: boolean; earned_time?: number }> = {};
  try {
    const raw = JSON.parse(fs.readFileSync(file, 'utf8'));
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      existing = raw as typeof existing;
    }
  } catch {
    /* none */
  }
  const out: Record<
    string,
    { displayName: string; description: string; earned: boolean; earned_time?: number }
  > = {};
  rows.forEach((row, i) => {
    const suffix = /_(\d+)$/.exec(row.name);
    const id = suffix ? suffix[1] : String(i + 1);
    const prev = existing[id];
    out[id] = {
      displayName: row.displayName || row.name,
      description: row.description || '',
      earned: prev?.earned === true,
    };
    if (prev?.earned_time) out[id].earned_time = prev.earned_time;
  });
  fs.mkdirSync(destDir, { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(out, null, 2)}\n`);
}

export function upcProductDirsForExe(exePath: string, appData = process.env.APPDATA ?? ''): string[] {
  const root = path.join(appData, 'Goldberg UplayEmu Saves');
  if (!fs.existsSync(root)) return [];
  const stem = path.basename(exePath).replace(/\.exe$/i, '').toLowerCase();
  const hits: string[] = [];
  for (const name of fs.readdirSync(root)) {
    const dir = path.join(root, name);
    let names: string[];
    try {
      if (!fs.statSync(dir).isDirectory()) continue;
      names = fs.readdirSync(dir);
    } catch {
      continue;
    }
    if (names.some((n) => n.toLowerCase().startsWith(stem))) hits.push(dir);
  }
  return hits;
}

function dllContainsGoldbergUpc(dllPath: string): boolean {
  try {
    const fd = fs.openSync(dllPath, 'r');
    try {
      const buf = Buffer.alloc(UPC_DLL_SCAN_BYTES);
      const n = fs.readSync(fd, buf, 0, buf.length, 0);
      const text = buf.toString('latin1', 0, n);
      return text.includes('Goldberg UplayEmu Saves') || text.includes('Mr_Goldberg');
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return false;
  }
}

function hasName(dir: string, name: string): boolean {
  if (!fs.existsSync(dir)) return false;
  return fs.readdirSync(dir).some((n) => n.toLowerCase() === name.toLowerCase());
}

function findUpcDir(exePath: string): string | null {
  for (const dir of detectScanDirs(exePath)) {
    if (UPC_LOADERS.some((n) => hasName(dir, n))) return dir;
  }
  return null;
}

function installDll(gameDir: string, src: string, name: string): void {
  const live = path.join(gameDir, name);
  const bak = `${live}.bak`;
  if (fs.existsSync(live)) {
    if (!fs.existsSync(bak)) fs.renameSync(live, bak);
    else fs.rmSync(live);
  }
  fs.copyFileSync(src, live);
}

export function isGoldbergUpc(exePath: string): boolean {
  const dir = findUpcDir(exePath);
  if (!dir) return false;
  return (
    dllContainsGoldbergUpc(path.join(dir, 'upc_r2_loader64.dll')) ||
    dllContainsGoldbergUpc(path.join(dir, 'upc_r2_loader.dll'))
  );
}

/** Backup + copy Goldberg upc_r2 next to a custom Uplay loader. Never patches `source: steam`. */
export function ensureGoldbergUpcForGame(opts: {
  exePath: string;
  source: string;
  cacheDir?: string;
}): { gameDir: string; installed: boolean } {
  const gameDir = findUpcDir(opts.exePath) ?? path.dirname(opts.exePath);
  if (opts.source === 'steam') {
    return { gameDir, installed: false };
  }
  if (isGoldbergUpc(opts.exePath)) {
    return { gameDir, installed: false };
  }
  const need64 =
    hasName(gameDir, 'upc_r2_loader64.dll') || hasName(gameDir, 'upc_r2_loader64.dll.bak');
  const need32 = hasName(gameDir, 'upc_r2_loader.dll') || hasName(gameDir, 'upc_r2_loader.dll.bak');
  if (!need64 && !need32) {
    return { gameDir, installed: false };
  }
  const cache = opts.cacheDir;
  if (!cache) {
    throw new Error('Goldberg Uplay cache is missing upc_r2_loader');
  }
  if (need64) {
    const src64 = path.join(cache, 'upc_r2_loader64.dll');
    if (!fs.existsSync(src64)) {
      throw new Error('Goldberg Uplay cache is missing upc_r2_loader64.dll');
    }
    installDll(gameDir, src64, 'upc_r2_loader64.dll');
  }
  if (need32) {
    const src32 = path.join(cache, 'upc_r2_loader.dll');
    if (!fs.existsSync(src32)) {
      throw new Error('Goldberg Uplay cache is missing upc_r2_loader.dll');
    }
    installDll(gameDir, src32, 'upc_r2_loader.dll');
  }
  return { gameDir, installed: true };
}

function seedUpcCacheFromGame(exePath: string, dest: string): void {
  const dir = findUpcDir(exePath);
  if (!dir) return;
  fs.mkdirSync(dest, { recursive: true });
  for (const name of ['upc_r2_loader64.dll', 'upc_r2_loader.dll']) {
    const src = path.join(dir, name);
    const out = path.join(dest, name);
    if (fs.existsSync(src) && !fs.existsSync(out) && dllContainsGoldbergUpc(src)) {
      fs.copyFileSync(src, out);
    }
  }
}

export function defaultUpcCacheDir(): string {
  return path.join(process.env.APPDATA ?? '', 'Glint', 'cache', 'upc');
}

export function enableUpcForGame(
  exePath: string,
  appData = process.env.APPDATA ?? '',
  cacheDir?: string,
): void {
  const cache = cacheDir ?? defaultUpcCacheDir();
  if (isGoldbergUpc(exePath)) {
    seedUpcCacheFromGame(exePath, cache);
  } else {
    ensureGoldbergUpcForGame({ exePath, source: 'custom', cacheDir: cache });
  }
  for (const dir of detectScanDirs(exePath)) {
    for (const ini of ['upc_r2.ini', 'uplay_r2.ini']) {
      const p = path.join(dir, ini);
      if (fs.existsSync(p)) enableUpcAchievementsIni(p);
    }
  }
  const rows = readSchemaRows(schemaDirForExe(exePath));
  if (rows.length === 0) return;
  for (const dest of upcProductDirsForExe(exePath, appData)) {
    writeUpcAchievementsJson(dest, rows);
  }
}

function readSchemaRows(
  gameDir: string,
): { name: string; displayName?: string; description?: string }[] {
  const file = path.join(gameDir, 'steam_settings', 'achievements.json');
  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) return [];
  const rows: { name: string; displayName?: string; description?: string }[] = [];
  for (const row of raw) {
    if (!row || typeof row !== 'object') continue;
    const name = (row as { name?: unknown }).name;
    if (typeof name !== 'string' || !name) continue;
    const displayName = (row as { displayName?: unknown }).displayName;
    const description = (row as { description?: unknown }).description;
    rows.push({
      name,
      displayName: typeof displayName === 'string' ? displayName : undefined,
      description: typeof description === 'string' ? description : undefined,
    });
  }
  return rows;
}

export function bridgeUpcUnlock(opts: {
  exePath: string;
  inId: string;
  earnedTimeSec?: number;
  appData?: string;
}): string | null {
  const appId = resolveSteamAppId(opts.exePath);
  if (!appId) return null;
  const steamName = mapUpcInId(opts.inId, readSchemaNames(schemaDirForExe(opts.exePath)));
  if (!steamName) return null;
  const dest = gseUnlockPath(appId, opts.appData ?? process.env.APPDATA ?? '');
  mergeGseUnlock(dest, steamName, opts.earnedTimeSec ?? Math.floor(Date.now() / 1000));
  return steamName;
}
