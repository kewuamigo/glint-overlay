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

  const hasSteam =
    lower.has('steam_api64.dll') ||
    lower.has('steam_api.dll') ||
    lower.has('steam_api64.dll.bak') ||
    lower.has('steam_api.dll.bak');
  const hasGse =
    lower.has('steam_settings') ||
    names.some((n) => n.toLowerCase().includes('gse')) ||
    fs.existsSync(path.join(dir, 'steam_settings'));
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
