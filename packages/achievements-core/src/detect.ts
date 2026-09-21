import fs from 'node:fs';
import path from 'node:path';
import {
  listAchievementsForGame,
  listTrackedGames,
  upsertTrackedGame,
  type AchievementSource,
  type TrackedGame,
} from './store.js';

export type EmuGuide = {
  kind: 'gse' | 'epic-emu' | 'epic-codex' | 'steam-official' | 'epic-official' | 'unknown';
  title: string;
  downloadUrl: string;
  steps: string[];
};

export type EpicEosVendor = 'nemirtingas' | 'codex' | 'unknown';

export type DetectResult = {
  platform: 'steam' | 'epic' | 'unknown';
  supported: boolean;
  markers: string[];
  guide: EmuGuide;
  /** Who replaced EOSSDK (Epic cracks). */
  epicVendor?: EpicEosVendor;
  /** Only Nemirtingas needs -epicapp / sandbox launch args. */
  offerEpicEmuLaunch?: boolean;
};

export const GUIDES: Record<string, EmuGuide> = {
  gse: {
    kind: 'gse',
    title: 'Install Goldberg / GSE for Steam achievements',
    downloadUrl: 'https://github.com/Detanup01/gbe_fork/releases',
    steps: [
      'Download a GSE / Goldberg Steam Emulator release.',
      'In the game folder, rename steam_api64.dll to steam_api64.dll.bak (or steam_api.dll).',
      'Copy the emulator DLLs and config next to the game executable.',
      'Generate achievements schema (generate_emu_config / GSE tools) for the Steam AppID.',
      'Play once; Glint watches %APPDATA%\\GSE Saves for unlocks.',
    ],
  },
  'epic-emu': {
    kind: 'epic-emu',
    title: 'Epic emulator achievements (Nemirtingas)',
    downloadUrl: 'https://pserban93.github.io/Achievements-Docs/platforms.html',
    steps: [
      'Nemirtingas EOSSDK detected (nepice_settings / NemirtingasEpicEmu.json).',
      'Launch with EpicEmu sandbox args when the launcher offers them.',
      'Play once; unlocks appear under %APPDATA%\\NemirtingasEpicEmu.',
      'Docs: https://pserban93.github.io/Achievements-Docs/platforms.html',
    ],
  },
  'epic-codex': {
    kind: 'epic-codex',
    title: 'Epic CODEX / EOS stub achievements',
    downloadUrl: 'https://pserban93.github.io/Achievements-Docs/platforms.html',
    steps: [
      'CODEX (or similar) EOSSDK stub detected — no Nemirtingas AppData path.',
      'Sync achievement definitions from the library (Epic public API).',
      'Play with the overlay/metrics inject; unlocks are hooked from EOS_Achievements_UnlockAchievements.',
      'Do not use Nemirtingas launch args with CODEX.',
    ],
  },
  'steam-official': {
    kind: 'steam-official',
    title: 'Steam official local stats',
    downloadUrl: 'https://store.steampowered.com/',
    steps: [
      'Install and sign in to Steam.',
      'Play the game through Steam so appcache\\stats is written.',
      'For cracked/offline builds without Steam, use the GSE guide instead.',
    ],
  },
  'epic-official': {
    kind: 'epic-official',
    title: 'Epic Games Launcher (official)',
    downloadUrl: 'https://store.epicgames.com/',
    steps: [
      'Install Epic Games Launcher and own the title.',
      'Epic account linking / polling lands in a later pass; for offline/emu builds use NemirtingasEpicEmu.',
      'See https://pserban93.github.io/Achievements-Docs/platforms.html for Epic official flow.',
    ],
  },
  unknown: {
    kind: 'unknown',
    title: 'Unsupported or unknown achievement backend',
    downloadUrl: 'https://pserban93.github.io/Achievements-Docs/platforms.html',
    steps: [
      'No Steamworks or EOS SDK DLLs were found next to the executable.',
      'If this is a Steam game, install GSE/Goldberg.',
      'If this is an Epic game, install NemirtingasEpicEmu.',
      'Docs: https://pserban93.github.io/Achievements-Docs/platforms.html',
    ],
  },
};

const BIN_DIR_NAMES = new Set([
  'bin',
  'bin64',
  'binaries',
  'win64',
  'win32',
  'x64',
  'x86',
  'shipping',
]);
const GSE_DLL_SCAN_BYTES = 2 * 1024 * 1024;
const STEAM_DLL_NAMES = new Set([
  'steam_api64.dll',
  'steam_api.dll',
  'steam_api64.dll.bak',
  'steam_api.dll.bak',
]);
const SKIP_WALK_DIRS = new Set([
  'content',
  'intermediate',
  'saved',
  'deriveddatacache',
  'movies',
  '.git',
  'node_modules',
  '__pycache__',
]);
const SKIP_EXE_DIRS = new Set([
  ...SKIP_WALK_DIRS,
  'engine',
  'easyanticheat',
  'easyanticheat_eos',
  '_commonredist',
  'redist',
  'directx',
]);
const SKIP_EXE_RE =
  /crash|unins\d*|vcredist|eacsetup|easyanticheat|crashpad|uninstall|dxsetup|dotnet/i;
const STEAM_WALK_DEPTH = 8;
const EXE_WALK_DEPTH = 6;

function hasEngineDir(root: string): boolean {
  return fs.existsSync(path.join(root, 'Engine'));
}

/** Walk up Binaries/Win64 (etc.) and prefer the UE root that contains Engine/. */
export function gameRootFromExe(exePath: string): string {
  let dir = path.dirname(path.resolve(exePath));
  for (let i = 0; i < 8; i++) {
    if (!BIN_DIR_NAMES.has(path.basename(dir).toLowerCase())) break;
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  if (hasEngineDir(dir)) return dir;
  const parent = path.dirname(dir);
  if (parent !== dir && hasEngineDir(parent)) return parent;
  return dir;
}

function findSteamDllDirs(root: string, maxDepth = STEAM_WALK_DEPTH): string[] {
  const out: string[] = [];
  const walk = (dir: string, depth: number) => {
    let names: string[];
    try {
      names = fs.readdirSync(dir);
    } catch {
      return;
    }
    const lower = names.map((n) => n.toLowerCase());
    if (lower.some((n) => STEAM_DLL_NAMES.has(n)) || lower.includes('steam_settings')) {
      out.push(dir);
    }
    if (depth <= 0) return;
    for (const name of names) {
      if (SKIP_WALK_DIRS.has(name.toLowerCase())) continue;
      const full = path.join(dir, name);
      try {
        if (fs.statSync(full).isDirectory()) walk(full, depth - 1);
      } catch {
        /* skip */
      }
    }
  };
  walk(root, maxDepth);
  return out;
}

function scoreGameExe(root: string, exe: string): number {
  const rel = path.relative(root, exe).replaceAll('\\', '/').toLowerCase();
  const base = path.basename(exe).toLowerCase();
  const stem = base.replace(/\.exe$/, '').replace(/[^a-z0-9]+/g, '');
  const rootKey = path.basename(root).toLowerCase().replace(/[^a-z0-9]+/g, '');
  let score = 0;
  if (base.endsWith('-win64-shipping.exe')) score += 50;
  if (rel.includes('binaries/win64/')) score += 30;
  else if (rel.includes('/win64/')) score += 10;
  if (rootKey && (stem.includes(rootKey) || rootKey.includes(stem))) score += 20;
  if (SKIP_EXE_RE.test(base)) score -= 80;
  return score;
}

/** Shipping / Binaries/Win64 exe under a user-picked install folder. */
export function findGameExeInFolder(root: string): string | null {
  if (!fs.existsSync(root)) return null;
  const hits: string[] = [];
  const walk = (dir: string, depth: number) => {
    let names: string[];
    try {
      names = fs.readdirSync(dir);
    } catch {
      return;
    }
    for (const name of names) {
      const full = path.join(dir, name);
      const lower = name.toLowerCase();
      try {
        const st = fs.statSync(full);
        if (st.isDirectory()) {
          if (depth > 0 && !SKIP_EXE_DIRS.has(lower)) walk(full, depth - 1);
        } else if (lower.endsWith('.exe') && !SKIP_EXE_RE.test(lower)) {
          hits.push(full);
        }
      } catch {
        /* skip */
      }
    }
  };
  walk(root, EXE_WALK_DEPTH);
  if (hits.length === 0) return null;
  hits.sort((a, b) => scoreGameExe(root, b) - scoreGameExe(root, a));
  return hits[0] ?? null;
}

export function detectScanDirs(exePath: string): string[] {
  const dir = path.dirname(exePath);
  const dirs = [dir];
  if (BIN_DIR_NAMES.has(path.basename(dir).toLowerCase())) {
    dirs.push(path.dirname(dir));
  }
  const canon = (d: string) =>
    process.platform === 'win32' ? path.resolve(d).toLowerCase() : path.resolve(d);
  const seen = new Set(dirs.map(canon));
  for (const extra of findSteamDllDirs(gameRootFromExe(exePath))) {
    const key = canon(extra);
    if (seen.has(key)) continue;
    seen.add(key);
    dirs.push(extra);
  }
  return dirs;
}

function readAppIdFile(filePath: string): string | null {
  try {
    const t = fs.readFileSync(filePath, 'utf8').trim();
    return /^\d+$/.test(t) ? t : null;
  } catch {
    return null;
  }
}

function readIniAppId(filePath: string): string | null {
  let text: string;
  try {
    text = fs.readFileSync(filePath, 'utf8');
  } catch {
    return null;
  }
  let section = '';
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#') || line.startsWith(';')) continue;
    const sec = /^\[([^\]]+)\]$/.exec(line);
    if (sec) {
      section = sec[1].toLowerCase();
      continue;
    }
    if (section === 'app::dlcs') continue;
    const m = /^(?:appid|app_id)\s*=\s*(\d+)/i.exec(line);
    if (m) return m[1];
  }
  return null;
}

export function resolveSteamAppId(exePath: string): string | null {
  for (const dir of detectScanDirs(exePath)) {
    const id =
      readAppIdFile(path.join(dir, 'steam_appid.txt')) ??
      readAppIdFile(path.join(dir, 'steam_settings', 'steam_appid.txt')) ??
      readIniAppId(path.join(dir, 'configs.app.ini')) ??
      readIniAppId(path.join(dir, 'steam_settings', 'configs.app.ini')) ??
      readIniAppId(path.join(dir, 'steam_api.ini'));
    if (id) return id;
  }
  return null;
}

function dllContainsGseMarker(dllPath: string): boolean {
  try {
    const fd = fs.openSync(dllPath, 'r');
    try {
      const buf = Buffer.alloc(GSE_DLL_SCAN_BYTES);
      const n = fs.readSync(fd, buf, 0, buf.length, 0);
      const text = buf.toString('latin1', 0, n);
      return text.includes('gbe_fork') || text.includes('GSE Client API');
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return false;
  }
}

function scanSteamGse(dir: string): { hasSteam: boolean; hasGse: boolean } {
  const names = fs.existsSync(dir) ? fs.readdirSync(dir) : [];
  const lower = new Set(names.map((n) => n.toLowerCase()));
  return {
    hasSteam:
      lower.has('steam_api64.dll') ||
      lower.has('steam_api.dll') ||
      lower.has('steam_api64.dll.bak') ||
      lower.has('steam_api.dll.bak'),
    hasGse:
      lower.has('steam_settings') ||
      names.some((n) => n.toLowerCase().includes('gse')) ||
      fs.existsSync(path.join(dir, 'steam_settings')) ||
      dllContainsGseMarker(path.join(dir, 'steam_api64.dll')) ||
      dllContainsGseMarker(path.join(dir, 'steam_api.dll')),
  };
}

function scanUplay(dir: string): boolean {
  const names = fs.existsSync(dir) ? fs.readdirSync(dir) : [];
  return names.some((n) => {
    const l = n.toLowerCase();
    return (
      l === 'upc_r2.ini' ||
      l === 'uplay_r2.ini' ||
      (l.startsWith('upc_r2_loader') && l.endsWith('.dll'))
    );
  });
}

/** Identify EOSSDK replacement vendor from folder markers only (no powershell PE). */
export function detectEosVendor(gameDir: string): EpicEosVendor {
  const names = fs.existsSync(gameDir) ? fs.readdirSync(gameDir) : [];
  const lower = new Set(names.map((n) => n.toLowerCase()));
  if (
    lower.has('nemirtingasepicemu.json') ||
    fs.existsSync(path.join(gameDir, 'nepice_settings'))
  ) {
    return 'nemirtingas';
  }
  if (lower.has('eossdk-win64-shipping.cdx') || lower.has('eossdk-win32-shipping.cdx')) {
    return 'codex';
  }
  return 'unknown';
}

export function detectExe(exePath: string): DetectResult {
  const dir = path.dirname(exePath);
  const markers: string[] = [];
  const names = fs.existsSync(dir) ? fs.readdirSync(dir) : [];
  const lower = new Set(names.map((n) => n.toLowerCase()));

  let hasSteam = false;
  let hasGse = false;
  let hasUplay = false;
  for (const scanDir of detectScanDirs(exePath)) {
    const hit = scanSteamGse(scanDir);
    hasSteam = hasSteam || hit.hasSteam;
    hasGse = hasGse || hit.hasGse;
    hasUplay = hasUplay || scanUplay(scanDir);
  }

  const hasEos =
    lower.has('eossdk-win64-shipping.dll') ||
    lower.has('eossdk-win64-shipping.cdx') ||
    lower.has('eossdk-win32-shipping.dll') ||
    fs.existsSync(path.join(dir, 'nepice_settings'));
  const hasEpicEmuFiles =
    lower.has('epic_emu.ini') ||
    lower.has('nemirtingasepicemu.json') ||
    lower.has('fetch_epic_achievements.bat') ||
    fs.existsSync(path.join(dir, 'nepice_settings')) ||
    fs.existsSync(path.join(dir, 'epic_emu.ini'));

  if (hasSteam) markers.push('steam_api');
  if (hasGse) markers.push('gse');
  if (hasUplay) markers.push('uplay');
  if (hasEos) markers.push('eossdk');
  if (hasEpicEmuFiles) markers.push('epic_emu');

  if (hasSteam && hasGse) {
    return { platform: 'steam', supported: true, markers, guide: GUIDES.gse };
  }
  if (hasSteam && !hasGse) {
    return { platform: 'steam', supported: false, markers, guide: GUIDES.gse };
  }

  if (hasEos || hasEpicEmuFiles) {
    const epicVendor = detectEosVendor(dir);
    if (epicVendor === 'codex') markers.push('codex');
    if (epicVendor === 'nemirtingas') markers.push('nemirtingas');
    const offerEpicEmuLaunch = epicVendor === 'nemirtingas';
    const supported =
      epicVendor === 'codex' ||
      epicVendor === 'nemirtingas' ||
      (hasEos && hasEpicEmuFiles);
    const guide =
      epicVendor === 'codex'
        ? GUIDES['epic-codex']
        : epicVendor === 'nemirtingas'
          ? GUIDES['epic-emu']
          : GUIDES['epic-emu'];
    return {
      platform: 'epic',
      supported,
      markers,
      guide,
      epicVendor,
      offerEpicEmuLaunch,
    };
  }

  return {
    platform: 'unknown',
    supported: false,
    markers,
    guide: GUIDES.unknown,
  };
}

export function getEmuGuide(kind: string): EmuGuide {
  return GUIDES[kind] ?? GUIDES.unknown;
}

export function trackGameFromExe(exePath: string): DetectResult {
  const detect = detectExe(exePath);
  const base = path.basename(exePath);
  const name = base.replace(/\.exe$/i, '');
  let source: AchievementSource = 'steam-official';
  let id = `custom:${base.toLowerCase()}`;
  let platform = detect.platform === 'unknown' ? 'unknown' : detect.platform;

  if (detect.platform === 'steam') {
    source = detect.supported ? 'gse' : 'steam-official';
    id = `steam-exe:${base.toLowerCase()}`;
  } else if (detect.platform === 'epic') {
    source = detect.supported ? 'epic-emu' : 'epic-official';
    id = `epic-exe:${base.toLowerCase()}`;
  }

  upsertTrackedGame({
    id,
    name,
    platform,
    source,
    exe_path: exePath,
    process_name: base,
  });
  return detect;
}

/** Match overlay session exe to a tracked game. Prefer rows that already have defs. */
export function findGameForProcess(
  exePath: string,
  exeName: string,
): TrackedGame | null {
  const games = listTrackedGames();
  const nameLower = exeName.toLowerCase();
  const pathLower = exePath.toLowerCase();
  const matches = games.filter(
    (g) =>
      (g.exe_path && g.exe_path.toLowerCase() === pathLower) ||
      (g.process_name && g.process_name.toLowerCase() === nameLower),
  );
  if (matches.length === 0) return null;
  return (
    matches.find((g) => listAchievementsForGame(g.id).length > 0) ?? matches[0]
  );
}

export { listTrackedGames, listAchievementsForGame };
