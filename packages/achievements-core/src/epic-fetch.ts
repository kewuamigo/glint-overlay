import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { getAchievementsDb } from './store.js';
import { upsertAchievementDef } from './store.js';

const EPIC_ACH_API =
  'https://api.epicgames.dev/epic/achievements/v1/public/achievements/product';
const EPIC_PRODUCT =
  'https://store-content.ak.epicgames.com/api/en-US/content/products';

export function toEpicProductSlug(name: string): string {
  return name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^\w\s-]/g, '')
    .replace(/[\s_]+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '');
}

export function getStoredEpicNamespace(gameId: string): string | null {
  const row = getAchievementsDb()
    .prepare(`SELECT value FROM sync_state WHERE key = ?`)
    .get(`epic_ns:${gameId}`) as { value: string } | undefined;
  return row?.value ?? null;
}

export function setStoredEpicNamespace(gameId: string, namespace: string): void {
  getAchievementsDb()
    .prepare(
      `INSERT INTO sync_state (key, value) VALUES (?, ?)
       ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
    )
    .run(`epic_ns:${gameId}`, namespace.trim());
}

/** Scan small text files for -epicapp= / epicsandboxid= hex ids. */
export function discoverNamespaceInDir(gameDir: string): string | null {
  if (!fs.existsSync(gameDir)) return null;
  const hex = /(?:-epicapp=|-epicsandboxid=|sandboxId["\s:=]+|namespace["\s:=]+)([a-f0-9]{32})/i;
  for (const name of fs.readdirSync(gameDir)) {
    const full = path.join(gameDir, name);
    let st: fs.Stats;
    try {
      st = fs.statSync(full);
    } catch {
      continue;
    }
    if (!st.isFile() || st.size > 256_000) continue;
    if (!/\.(ini|txt|bat|cfg|json|xml)$/i.test(name) && !/launcher/i.test(name)) {
      continue;
    }
    try {
      const text = fs.readFileSync(full, 'utf8');
      const m = text.match(hex);
      if (m?.[1]) return m[1].toLowerCase();
    } catch {
      // skip binary
    }
  }
  return null;
}

export async function resolveEpicNamespace(
  gameId: string,
  gameName: string,
  gameDir: string,
): Promise<string | null> {
  const stored = getStoredEpicNamespace(gameId);
  if (stored) return stored;

  const fromDir = discoverNamespaceInDir(gameDir);
  if (fromDir) {
    setStoredEpicNamespace(gameId, fromDir);
    return fromDir;
  }

  const slug = toEpicProductSlug(gameName);
  if (!slug) return null;
  try {
    const res = await fetch(`${EPIC_PRODUCT}/${encodeURIComponent(slug)}`);
    if (!res.ok) return null;
    const json = (await res.json()) as { namespace?: string };
    if (typeof json.namespace === 'string' && json.namespace.length >= 16) {
      setStoredEpicNamespace(gameId, json.namespace);
      return json.namespace;
    }
  } catch {
    // ignore
  }
  return null;
}

type EpicAchPayload = {
  achievements?: Array<{
    achievement?: {
      name?: string;
      unlockedDisplayName?: string;
      unlockedDescription?: string;
      lockedDisplayName?: string;
      lockedDescription?: string;
      unlockedIconLink?: string;
      lockedIconLink?: string;
      hidden?: boolean;
    };
  }>;
};

async function fetchEpicAchievementsPayload(
  namespace: string,
): Promise<EpicAchPayload> {
  const url = `${EPIC_ACH_API}/${encodeURIComponent(namespace)}/locale/en?includeAchievements=true`;
  const res = await fetch(url, { headers: { Accept: 'application/json' } });
  if (!res.ok) {
    throw new Error(`Epic achievements API ${res.status}`);
  }
  return (await res.json()) as EpicAchPayload;
}

function localeMap(value: string): Record<string, string> {
  const v = value || '';
  return {
    de: v,
    default: v,
    en: v,
    'es-ES': v,
    fr: v,
    it: v,
    ja: v,
    'pt-BR': v,
    ru: v,
    zh: v,
  };
}

/** Match fetch_epic_achievements.bat / All-in-One generator output for Nemirtingas. */
function epicPayloadToNepiceDb(json: EpicAchPayload): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = [];
  for (const item of json.achievements ?? []) {
    const a = item?.achievement;
    if (!a?.name) continue;
    const id = String(a.name);
    const unlockedName = String(
      a.unlockedDisplayName || a.lockedDisplayName || id,
    );
    const lockedName = String(
      a.lockedDisplayName || a.unlockedDisplayName || id,
    );
    const unlockedDesc = String(a.unlockedDescription || '');
    const lockedDesc = String(a.lockedDescription || '');
    out.push({
      AchievementId: id,
      UnlockedDisplayName: localeMap(unlockedName),
      UnlockedDescription: localeMap(unlockedDesc),
      LockedDisplayName: localeMap(lockedName),
      LockedDescription: localeMap(lockedDesc),
      FlavorText: { default: '' },
      UnlockedIconUrl: id,
      LockedIconUrl: `${id}_locked`,
      IsHidden: Boolean(a.hidden),
      StatsThresholds: [{ Name: id, Threshold: 0 }],
    });
  }
  return out;
}

/** Stable clean-profile Epic id (must match launcher -epicuserid). */
export const NEPICE_CLEAN_EPIC_ID = '69bfe74044409f2feb6f5e11c695a47a';
const NEPICE_CLEAN_PRODUCT_USER_ID = '0002a0c630d8c2127375e38a7d2c86a3';
const NEPICE_CLEAN_USERNAME = 'DefaultName';

/**
 * Docs setup for Nemirtingas: NemirtingasEpicEmu.json + nepice_settings/achievements_db.json.
 * Does not install EOSSDK DLLs.
 */
export async function writeNemirtingasSetup(
  gameDir: string,
  namespace: string,
  gameName: string,
): Promise<void> {
  const cfgPath = path.join(gameDir, 'NemirtingasEpicEmu.json');
  if (!fs.existsSync(cfgPath)) {
    fs.writeFileSync(
      cfgPath,
      `${JSON.stringify(
        {
          appid: namespace,
          gamename: gameName,
          language: 'en',
          savepath: 'appdata',
          username: NEPICE_CLEAN_USERNAME,
          unlock_dlcs: true,
          disable_online_networking: true,
          enable_overlay: false,
        },
        null,
        2,
      )}\n`,
    );
  }

  const nepice = path.join(gameDir, 'nepice_settings');
  fs.mkdirSync(nepice, { recursive: true });

  // Newer Nemirtingas builds read nepice_settings/NemirtingasEpicEmu.json (nested).
  const nepiceCfgPath = path.join(nepice, 'NemirtingasEpicEmu.json');
  let nepiceCfg: Record<string, unknown> = {};
  if (fs.existsSync(nepiceCfgPath)) {
    try {
      nepiceCfg = JSON.parse(fs.readFileSync(nepiceCfgPath, 'utf8')) as Record<
        string,
        unknown
      >;
    } catch {
      nepiceCfg = {};
    }
  }
  const eos =
    nepiceCfg.EOSEmu && typeof nepiceCfg.EOSEmu === 'object'
      ? (nepiceCfg.EOSEmu as Record<string, unknown>)
      : {};
  const app =
    eos.Application && typeof eos.Application === 'object'
      ? (eos.Application as Record<string, unknown>)
      : {};
  app.AppId = namespace;
  app.SavePath = 'appdata';
  app.DisableOnlineNetworking = true;
  app.LogLevel = 'info';
  eos.Application = app;
  eos.User = {
    EpicId: NEPICE_CLEAN_EPIC_ID,
    Language: 'en',
    ProductUserId: NEPICE_CLEAN_PRODUCT_USER_ID,
    UserName: NEPICE_CLEAN_USERNAME,
  };
  const plugins =
    eos.Plugins && typeof eos.Plugins === 'object'
      ? (eos.Plugins as Record<string, unknown>)
      : {};
  plugins.Overlay = { DelayDetection: '5s', Enabled: false };
  eos.Plugins = plugins;
  nepiceCfg.EOSEmu = eos;
  fs.writeFileSync(nepiceCfgPath, `${JSON.stringify(nepiceCfg, null, 2)}\n`);

  const json = await fetchEpicAchievementsPayload(namespace);
  fs.writeFileSync(
    path.join(nepice, 'achievements_db.json'),
    `${JSON.stringify(epicPayloadToNepiceDb(json), null, 2)}\n`,
  );

  // Emu logs these paths but often never mkdir's — seed so unlocks have a target.
  const appdata =
    process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming');
  const saveDir = path.join(
    appdata,
    'NemirtingasEpicEmu',
    NEPICE_CLEAN_EPIC_ID,
    namespace,
  );
  fs.mkdirSync(saveDir, { recursive: true });
  const achPath = path.join(saveDir, 'achievements.json');
  const statsPath = path.join(saveDir, 'stats.json');
  if (!fs.existsSync(achPath)) fs.writeFileSync(achPath, '[]\n');
  if (!fs.existsSync(statsPath)) fs.writeFileSync(statsPath, '[]\n');
}

/** Fetch public Epic achievement schema and upsert locked+unlocked defs. */
export async function fetchAndImportEpicAchievements(
  gameId: string,
  namespace: string,
): Promise<number> {
  const json = await fetchEpicAchievementsPayload(namespace);
  const list = Array.isArray(json.achievements) ? json.achievements : [];
  let count = 0;
  for (const item of list) {
    const a = item?.achievement;
    if (!a?.name) continue;
    upsertAchievementDef({
      gameId,
      achievementId: String(a.name),
      title: String(a.unlockedDisplayName || a.lockedDisplayName || a.name),
      description: String(
        a.unlockedDescription || a.lockedDescription || '',
      ) || null,
      iconLocked: a.lockedIconLink ? String(a.lockedIconLink) : null,
      iconUnlocked: a.unlockedIconLink ? String(a.unlockedIconLink) : null,
    });
    count += 1;
  }
  return count;
}
