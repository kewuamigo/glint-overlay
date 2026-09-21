import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {
  recordUnlock,
  upsertAchievementDef,
  upsertTrackedGame,
  type AchievementSource,
} from './store.js';
import { detectExe, type DetectResult } from './detect.js';
import {
  fetchAndImportEpicAchievements,
  resolveEpicNamespace,
  writeNemirtingasSetup,
} from './epic-fetch.js';
import { listAchievementsForGame } from './store.js';

function readJson(filePath: string): unknown | null {
  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch {
    return null;
  }
}

function collectJsonFiles(dir: string): string[] {
  const out: string[] = [];
  const names = [
    'achievements.json',
    'achievements_db.json',
    'Achievement.json',
  ];
  for (const name of names) {
    const p = path.join(dir, name);
    if (fs.existsSync(p)) out.push(p);
  }
  const nepice = path.join(dir, 'nepice_settings');
  if (fs.existsSync(nepice)) {
    for (const name of names) {
      const p = path.join(nepice, name);
      if (fs.existsSync(p)) out.push(p);
    }
  }
  const steamSettings = path.join(dir, 'steam_settings');
  if (fs.existsSync(steamSettings)) {
    for (const name of names) {
      const p = path.join(steamSettings, name);
      if (fs.existsSync(p)) out.push(p);
    }
  }
  const epicOut = path.join(dir, 'epic_output', 'achievements_db.json');
  if (fs.existsSync(epicOut)) out.push(epicOut);
  return out;
}

function asIconUrl(value: unknown): string | null {
  if (typeof value !== 'string' || !value) return null;
  // Nepirtingas generator stores bare ids ("1", "1_locked") — not loadable in UI.
  if (/^(https?:|file:)/i.test(value)) return value;
  return null;
}

function ingestDefs(
  gameId: string,
  raw: unknown,
  applyUnlockState: boolean,
): number {
  let count = 0;

  const add = (
    achId: string,
    title: string,
    description: string | null,
    iconLocked: string | null,
    iconUnlocked: string | null,
    unlocked: boolean,
    unlockedAt: number | null,
  ) => {
    if (!achId) return;
    upsertAchievementDef({
      gameId,
      achievementId: achId,
      title: title || achId,
      description,
      iconLocked,
      iconUnlocked,
    });
    if (applyUnlockState && unlocked) {
      recordUnlock({
        gameId,
        achievementId: achId,
        title: title || achId,
        description,
        iconUnlocked,
        unlockedAt,
      });
    }
    count += 1;
  };

  if (Array.isArray(raw)) {
    for (const item of raw) {
      if (!item || typeof item !== 'object') continue;
      const row = item as Record<string, unknown>;
      const achId = String(
        row.achievement_id ?? row.AchievementId ?? row.id ?? row.name ?? '',
      );
      const titleObj = row.UnlockedDisplayName ?? row.unlockedDisplayName;
      const descObj = row.UnlockedDescription ?? row.unlockedDescription;
      const titleFromMap =
        titleObj && typeof titleObj === 'object'
          ? String(
              (titleObj as Record<string, unknown>).default ??
                (titleObj as Record<string, unknown>).en ??
                '',
            )
          : '';
      const descFromMap =
        descObj && typeof descObj === 'object'
          ? String(
              (descObj as Record<string, unknown>).default ??
                (descObj as Record<string, unknown>).en ??
                '',
            )
          : '';
      const title = String(
        row.title ??
          row.displayName ??
          row.Name ??
          (titleFromMap || undefined) ??
          row.description ??
          achId,
      );
      const description =
        typeof row.description === 'string'
          ? row.description
          : typeof row.Description === 'string'
            ? row.Description
            : descFromMap || null;
      const iconLocked = asIconUrl(
        row.icon_locked ?? row.LockedIcon_URL ?? row.LockedIconUrl,
      );
      const iconUnlocked = asIconUrl(
        row.icon_unlocked ??
          row.icon ??
          row.UnlockedIcon_URL ??
          row.UnlockedIconUrl,
      );
      const unlocked =
        row.unlocked === true ||
        row.unlocked === 1 ||
        row.earned === true ||
        row.earned === 1 ||
        Number(row.earned_time ?? 0) > 0 ||
        row.UnlockTime != null;
      const unlockedAt =
        typeof row.earned_time === 'number'
          ? row.earned_time * 1000
          : typeof row.UnlockTime === 'number'
            ? row.UnlockTime
            : null;
      add(achId, title, description, iconLocked, iconUnlocked, unlocked, unlockedAt);
    }
    return count;
  }

  if (raw && typeof raw === 'object') {
    for (const [achId, value] of Object.entries(raw as Record<string, unknown>)) {
      if (!value || typeof value !== 'object') {
        add(achId, achId, null, null, null, false, null);
        continue;
      }
      const v = value as Record<string, unknown>;
      const title = String(v.name ?? v.displayName ?? v.title ?? achId);
      const description = typeof v.description === 'string' ? v.description : null;
      const iconUnlocked = asIconUrl(v.icon ?? v.icon_unlocked ?? v.UnlockedIconUrl);
      const iconLocked = asIconUrl(v.icon_locked ?? v.LockedIconUrl);
      const unlocked =
        v.earned === true ||
        v.unlocked === true ||
        v.earned === 1 ||
        Number(v.earned_time ?? 0) > 0;
      const unlockedAt =
        typeof v.earned_time === 'number' ? v.earned_time * 1000 : null;
      add(achId, title, description, iconLocked, iconUnlocked, unlocked, unlockedAt);
    }
  }

  return count;
}

/** Import locked+unlocked defs from game folder and common AppData emu caches. */
export function importDefsFromGameDir(gameId: string, gameDir: string): number {
  let total = 0;
  for (const file of collectJsonFiles(gameDir)) {
    const raw = readJson(file);
    if (raw) total += ingestDefs(gameId, raw, true);
  }

  const gseRoot = path.join(
    process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming'),
    'GSE Saves',
  );
  if (fs.existsSync(gseRoot)) {
    for (const appId of fs.readdirSync(gseRoot)) {
      const ach = path.join(gseRoot, appId, 'achievements.json');
      if (!fs.existsSync(ach)) continue;
      // Only import if process/exe hints match later; for library sync we still
      // allow when gameId embeds appId or when only one file — skip broad import.
      if (gameId.includes(appId) || gameId.endsWith(`:${appId}`)) {
        const raw = readJson(ach);
        if (raw) total += ingestDefs(gameId, raw, true);
      }
    }
  }

  const epicRoot = path.join(
    process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming'),
    'NemirtingasEpicEmu',
  );
  if (fs.existsSync(epicRoot)) {
    for (const ns of fs.readdirSync(epicRoot)) {
      const base = path.join(epicRoot, ns);
      if (!fs.statSync(base).isDirectory()) continue;
      for (const name of ['achievements.json', 'achievements_db.json']) {
        const file = path.join(base, name);
        if (!fs.existsSync(file)) continue;
        if (gameId.includes(ns)) {
          const raw = readJson(file);
          if (raw) total += ingestDefs(gameId, raw, true);
        }
      }
    }
  }

  return total;
}

export function resolveLibraryExePath(
  exe: string,
  installPath: string,
): string | null {
  const trimmedExe = exe.trim();
  const trimmedInstall = installPath.trim();
  if (!trimmedExe) return null;
  if (/[/\\]/.test(trimmedExe) && fs.existsSync(trimmedExe)) return trimmedExe;
  if (trimmedInstall) {
    const joined = path.join(trimmedInstall, path.basename(trimmedExe));
    if (fs.existsSync(joined)) return joined;
    const joinedRaw = path.join(trimmedInstall, trimmedExe);
    if (fs.existsSync(joinedRaw)) return joinedRaw;
  }
  return fs.existsSync(trimmedExe) ? trimmedExe : null;
}

export async function syncLibraryGame(input: {
  id: string;
  name: string;
  exe: string;
  install_path: string;
}): Promise<DetectResult | null> {
  const exePath = resolveLibraryExePath(input.exe, input.install_path);
  if (!exePath) return null;

  const detect = detectExe(exePath);
  const base = path.basename(exePath);
  let source: AchievementSource = 'steam-official';
  if (detect.platform === 'steam') {
    source = detect.supported ? 'gse' : 'steam-official';
  } else if (detect.platform === 'epic') {
    source = detect.supported ? 'epic-emu' : 'epic-official';
  }

  upsertTrackedGame({
    id: input.id,
    name: input.name,
    platform: detect.platform === 'unknown' ? 'unknown' : detect.platform,
    source,
    exe_path: exePath,
    process_name: base,
  });

  const gameDir = path.dirname(exePath);
  let imported = importDefsFromGameDir(input.id, gameDir);

  if (detect.platform === 'epic') {
    try {
      const ns = await resolveEpicNamespace(input.id, input.name, gameDir);
      if (ns) {
        // Always refresh from Epic API so CDN icon URLs win over nepice bare ids.
        imported = await fetchAndImportEpicAchievements(input.id, ns);
        if (detect.epicVendor === 'nemirtingas') {
          await writeNemirtingasSetup(gameDir, ns, input.name);
        }
      }
    } catch {
      // keep local-only defs
    }
  }

  // Prefer showing locked+unlocked count after fetch
  if (imported === 0) {
    imported = listAchievementsForGame(input.id).length;
  }

  return detect;
}
