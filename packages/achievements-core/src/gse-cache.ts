import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { detectExe, detectScanDirs } from './detect.js';

export const GSE_LATEST_API =
  'https://api.github.com/repos/Detanup01/gbe_fork/releases/latest';

export const GSE_ASSET_NAME = 'emu-win-release.7z';

export function gseAssetUrl(tag: string): string {
  return `https://github.com/Detanup01/gbe_fork/releases/download/${tag}/${GSE_ASSET_NAME}`;
}

export const GSE_ASSET_URL = `https://github.com/Detanup01/gbe_fork/releases/latest/download/${GSE_ASSET_NAME}`;

/** Standalone LZMA/LZMA2 extractor; used only when 7z is not on PATH. */
const SEVEN_ZR_URL = 'https://www.7-zip.org/a/7zr.exe';

const STEAM_API_NAMES = [
  'steam_api64.dll',
  'steam_api.dll',
  'steam_api64.dll.bak',
  'steam_api.dll.bak',
];

function psQuote(s: string): string {
  return `'${s.replace(/'/g, "''")}'`;
}

function scoreDllDir(dir: string): number {
  const p = dir.replace(/\\/g, '/').toLowerCase();
  let s = 0;
  if (p.includes('/debug')) s -= 50;
  if (p.includes('/release')) s += 20;
  if (/(^|\/)x64(\/|$)/.test(p)) s += 10;
  if (p.includes('experimental')) s -= 5;
  if (p.includes('regular')) s += 5;
  return s;
}

function dirWithDll(root: string): string | null {
  if (!fs.existsSync(root)) return null;
  const hits: string[] = [];
  const walk = (dir: string, depth: number) => {
    if (fs.existsSync(path.join(dir, 'steam_api64.dll'))) hits.push(dir);
    if (depth <= 0) return;
    let names: string[];
    try {
      names = fs.readdirSync(dir);
    } catch {
      return;
    }
    for (const name of names) {
      const sub = path.join(dir, name);
      try {
        if (fs.statSync(sub).isDirectory()) walk(sub, depth - 1);
      } catch {
        /* skip */
      }
    }
  };
  walk(root, 6);
  if (hits.length === 0) return null;
  hits.sort((a, b) => scoreDllDir(b) - scoreDllDir(a));
  return hits[0];
}

function defaultDest(tag: string): string {
  return path.join(process.env.APPDATA ?? '', 'Glint', 'cache', 'gse', tag);
}

export async function resolveLatestGseTag(): Promise<string> {
  const res = await fetch(GSE_LATEST_API, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!res.ok) {
    throw new Error(`GSE latest release failed: HTTP ${res.status}`);
  }
  const json = (await res.json()) as { tag_name?: string };
  const tag = json.tag_name?.trim() ?? '';
  if (!tag) {
    throw new Error('GSE latest release has no tag_name');
  }
  return tag;
}

function isZip(buf: Buffer): boolean {
  return buf.length >= 2 && buf[0] === 0x50 && buf[1] === 0x4b;
}

function resolve7z(): string | null {
  for (const cmd of ['7z', '7za', '7zr']) {
    try {
      execFileSync(cmd, ['i'], {
        stdio: 'ignore',
        windowsHide: true,
        timeout: 8000,
      });
      return cmd;
    } catch (e) {
      const err = e as NodeJS.ErrnoException;
      if (err.code === 'ENOENT') continue;
      return cmd;
    }
  }
  for (const p of [
    path.join(process.env['ProgramFiles'] ?? 'C:\\Program Files', '7-Zip', '7z.exe'),
    path.join(
      process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)',
      '7-Zip',
      '7z.exe',
    ),
  ]) {
    if (fs.existsSync(p)) return p;
  }
  return null;
}

async function ensure7zr(dest: string): Promise<string> {
  const cached = path.join(dest, '7zr.exe');
  if (fs.existsSync(cached)) return cached;
  const found = resolve7z();
  if (found) return found;
  const res = await fetch(SEVEN_ZR_URL);
  if (!res.ok) {
    throw new Error(`7zr download failed: HTTP ${res.status}`);
  }
  fs.writeFileSync(cached, Buffer.from(await res.arrayBuffer()));
  return cached;
}

function extractZip(zipPath: string, dest: string): void {
  execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `Expand-Archive -LiteralPath ${psQuote(zipPath)} -DestinationPath ${psQuote(dest)} -Force`,
    ],
    { windowsHide: true },
  );
}

function extract7z(exe: string, archivePath: string, dest: string): void {
  execFileSync(exe, ['x', archivePath, `-o${dest}`, '-y'], { windowsHide: true });
}

/** Download/extract latest GSE bits; reuse if steam_api64.dll is already cached. */
export async function ensureGseCache(
  url?: string,
  destDir?: string,
): Promise<string> {
  let dest = destDir;
  let assetUrl = url;
  if (!dest || !assetUrl) {
    const tag = await resolveLatestGseTag();
    dest = dest ?? defaultDest(tag);
    assetUrl = assetUrl ?? gseAssetUrl(tag);
  }
  const hit = dirWithDll(dest);
  if (hit) return hit;

  const res = await fetch(assetUrl);
  if (!res.ok) {
    throw new Error(`GSE download failed: HTTP ${res.status}`);
  }

  fs.mkdirSync(dest, { recursive: true });
  const buf = Buffer.from(await res.arrayBuffer());
  const zip = isZip(buf);
  const archivePath = path.join(dest, zip ? 'gse.zip' : 'gse.7z');
  fs.writeFileSync(archivePath, buf);

  if (zip) {
    extractZip(archivePath, dest);
  } else {
    extract7z(await ensure7zr(dest), archivePath, dest);
  }

  const found = dirWithDll(dest);
  if (!found) {
    throw new Error('GSE archive is missing steam_api64.dll');
  }
  return found;
}

function hasName(dir: string, name: string): boolean {
  if (!fs.existsSync(dir)) return false;
  return fs.readdirSync(dir).some((n) => n.toLowerCase() === name.toLowerCase());
}

function findSteamApiDir(exePath: string): string | null {
  for (const dir of detectScanDirs(exePath)) {
    if (STEAM_API_NAMES.some((n) => hasName(dir, n))) return dir;
  }
  return null;
}

function numericAppId(appId: string | null | undefined): string | null {
  const t = appId == null ? '' : String(appId).trim();
  return /^\d+$/.test(t) ? t : null;
}

function writeAppIdIfMissing(dir: string, appId: string): void {
  const file = path.join(dir, 'steam_settings', 'steam_appid.txt');
  if (fs.existsSync(file)) return;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${appId}\n`);
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

/** Backup + copy GSE next to a custom Steamworks game. Never patches `source: steam`. */
export async function ensureGseForGame(opts: {
  exePath: string;
  source: string;
  appId?: string | null;
  cacheDir?: string;
  /** Install steam_api64.dll even when the folder has no Steamworks DLL. */
  force?: boolean;
}): Promise<{ gameDir: string; installed: boolean }> {
  const gameDir = findSteamApiDir(opts.exePath) ?? path.dirname(opts.exePath);
  if (opts.source === 'steam') {
    return { gameDir, installed: false };
  }

  const detect = detectExe(opts.exePath);
  const watchable = detect.platform === 'steam' && detect.supported;
  const id = numericAppId(opts.appId);
  if (watchable) {
    if (id) {
      const settingsParent =
        detectScanDirs(opts.exePath).find((d) =>
          fs.existsSync(path.join(d, 'steam_settings')),
        ) ?? gameDir;
      writeAppIdIfMissing(settingsParent, id);
    }
    return { gameDir, installed: false };
  }

  const needsInstall = STEAM_API_NAMES.some((n) => hasName(gameDir, n));
  let need64 = hasName(gameDir, 'steam_api64.dll') || hasName(gameDir, 'steam_api64.dll.bak');
  const need32 = hasName(gameDir, 'steam_api.dll') || hasName(gameDir, 'steam_api.dll.bak');
  if (!needsInstall) {
    if (!opts.force) {
      return { gameDir, installed: false };
    }
    need64 = true;
  }
  if (!id) {
    throw new Error('GSE install requires a numeric AppID');
  }

  const cache = opts.cacheDir ?? (await ensureGseCache());
  if (need64) {
    const src64 = path.join(cache, 'steam_api64.dll');
    if (!fs.existsSync(src64)) {
      throw new Error('GSE cache is missing steam_api64.dll');
    }
    installDll(gameDir, src64, 'steam_api64.dll');
  }
  if (need32) {
    let src32 = path.join(cache, 'steam_api.dll');
    if (!fs.existsSync(src32)) {
      src32 = path.join(path.dirname(cache), 'Win32', 'steam_api.dll');
    }
    if (!fs.existsSync(src32)) {
      throw new Error('GSE cache is missing steam_api.dll');
    }
    installDll(gameDir, src32, 'steam_api.dll');
  }
  writeAppIdIfMissing(gameDir, id);
  return { gameDir, installed: true };
}
