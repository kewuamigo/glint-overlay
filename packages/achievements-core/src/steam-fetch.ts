import fs from 'node:fs';
import path from 'node:path';
import { upsertAchievementDef } from './store.js';

type SteamAchievement = {
  name?: string;
  displayName?: string;
  description?: string;
  icon?: string;
  icongray?: string;
  hidden?: number;
};

type SteamSchemaPayload = {
  game?: {
    availableGameStats?: {
      achievements?: SteamAchievement[];
    };
  };
};

type GseAchievement = {
  name: string;
  displayName: string;
  description: string;
  hidden: number;
};

/**
 * Write generate_emu_config-style achievements.json (JSON array).
 *
 * gbe_fork looks up SetAchievement() against each object's `name` (the Steam
 * API id). Icon pagination uses json.at(index), which throws on an object map
 * and crashed Crimson Desert. Omit icon paths — CDN URLs are not local files.
 */
export function writeGseAchievementsJson(
  gameDir: string,
  achievements: SteamAchievement[],
): void {
  const settings = path.join(gameDir, 'steam_settings');
  if (!fs.existsSync(settings)) return;
  const out: GseAchievement[] = [];
  for (const a of achievements) {
    if (!a?.name) continue;
    const apiName = String(a.name);
    out.push({
      name: apiName,
      displayName: a.displayName ? String(a.displayName) : apiName,
      description: a.description ? String(a.description) : '',
      hidden: a.hidden ? 1 : 0,
    });
  }
  fs.writeFileSync(
    path.join(settings, 'achievements.json'),
    `${JSON.stringify(out, null, 2)}\n`,
  );
}

/** Fetch Steam GetSchemaForGame and upsert defs under the library game id. */
export async function fetchAndImportSteamAchievements(
  gameId: string,
  appId: string,
  apiKey: string,
  gameDir?: string,
): Promise<number> {
  if (!apiKey?.trim()) {
    throw new Error(
      'Steam Web API key is missing. Save a Steam Web API key in Settings.',
    );
  }
  if (!appId?.trim()) {
    throw new Error('Steam AppID is missing.');
  }

  const url =
    `https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/` +
    `?key=${encodeURIComponent(apiKey.trim())}` +
    `&appid=${encodeURIComponent(appId.trim())}`;
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`Steam GetSchemaForGame ${res.status}`);
  }

  const json = (await res.json()) as SteamSchemaPayload;
  const list = json.game?.availableGameStats?.achievements ?? [];
  let count = 0;
  for (const a of list) {
    if (!a?.name) continue;
    upsertAchievementDef({
      gameId,
      achievementId: String(a.name),
      title: String(a.displayName || a.name),
      description: a.description ? String(a.description) : null,
      iconLocked: a.icongray ? String(a.icongray) : null,
      iconUnlocked: a.icon ? String(a.icon) : null,
    });
    count += 1;
  }
  if (gameDir) writeGseAchievementsJson(gameDir, list);
  return count;
}
