import {
  clearCoverManual,
  getCover,
  getDb,
  setCover,
  setCoverManual,
  type ArtSlot,
  type CoverRow,
} from './db.js';
import { loadSettings } from './settings.js';

export type GameArt = {
  icon?: string;
  grid?: string;
  hero?: string;
  /** SGDB logo URL; empty string in DB means looked up, none found. */
  logo?: string;
  iconMime?: string;
  gridMime?: string;
  heroMime?: string;
  logoMime?: string;
};

export type SgdbAsset = {
  id: number;
  url: string;
  thumb: string;
  mime: string;
  style: string;
  animated: boolean;
};

export type ArtKind = ArtSlot;

type GameRef = {
  id: string;
  name: string;
};

type AssetKind = 'grids' | 'icons' | 'heroes' | 'logos';

const SGDB = 'https://www.steamgriddb.com/api/v2';

const SLOT_TO_API: Record<ArtSlot, AssetKind> = {
  icon: 'icons',
  grid: 'grids',
  hero: 'heroes',
  logo: 'logos',
};

function steamCdnArt(appId: string): GameArt {
  const base = `https://cdn.cloudflare.steamstatic.com/steam/apps/${appId}`;
  return {
    icon: `${base}/capsule_231x87.jpg`,
    grid: `${base}/library_600x900.jpg`,
    hero: `${base}/library_hero.jpg`,
    iconMime: 'image/jpeg',
    gridMime: 'image/jpeg',
    heroMime: 'image/jpeg',
  };
}

function authHeaders(apiKey: string): HeadersInit {
  return { Authorization: `Bearer ${apiKey}` };
}

async function firstAsset(
  apiKey: string,
  kind: AssetKind,
  pathSuffix: string,
  query = '',
): Promise<{ url: string; mime: string } | null> {
  const res = await fetch(`${SGDB}/${kind}/${pathSuffix}${query}`, {
    headers: authHeaders(apiKey),
  });
  if (!res.ok) return null;
  const json = (await res.json()) as {
    data?: Array<{ url?: string; thumb?: string; mime?: string }>;
  };
  const first = json.data?.[0];
  const url = first?.url ?? first?.thumb;
  if (!url) return null;
  return { url, mime: first?.mime ?? '' };
}

async function searchGameIdByName(apiKey: string, name: string): Promise<number | null> {
  const term = name.trim();
  if (!term) return null;
  const res = await fetch(
    `${SGDB}/search/autocomplete/${encodeURIComponent(term)}`,
    { headers: authHeaders(apiKey) },
  );
  if (!res.ok) return null;
  const json = (await res.json()) as {
    data?: Array<{ id?: number }>;
  };
  const first = json.data?.[0];
  return typeof first?.id === 'number' ? first.id : null;
}

async function resolveSgdbPath(
  apiKey: string,
  game: GameRef,
): Promise<string | null> {
  if (game.id.startsWith('steam:')) {
    const appId = game.id.slice('steam:'.length);
    if (!appId) return null;
    return `steam/${encodeURIComponent(appId)}`;
  }
  const sgdbId = await searchGameIdByName(apiKey, game.name);
  if (sgdbId == null) return null;
  return `game/${sgdbId}`;
}

async function fetchArtBundle(
  apiKey: string,
  pathSuffix: string,
): Promise<GameArt> {
  const gridQ = '?dimensions=600x900&types=static,animated';
  const q = '?types=static,animated';
  const [icon, grid, hero, logo] = await Promise.all([
    firstAsset(apiKey, 'icons', pathSuffix, q),
    firstAsset(apiKey, 'grids', pathSuffix, gridQ),
    firstAsset(apiKey, 'heroes', pathSuffix, q),
    firstAsset(apiKey, 'logos', pathSuffix, q),
  ]);
  return {
    icon: icon?.url,
    grid: grid?.url,
    hero: hero?.url,
    logo: logo?.url ?? '',
    iconMime: icon?.mime || undefined,
    gridMime: grid?.mime || undefined,
    heroMime: hero?.mime || undefined,
    logoMime: logo?.mime || undefined,
  };
}

function mergeArt(primary: GameArt, fallback?: GameArt): GameArt {
  return {
    icon: primary.icon ?? fallback?.icon,
    grid: primary.grid ?? fallback?.grid,
    hero: primary.hero ?? fallback?.hero,
    logo: primary.logo || fallback?.logo || '',
    iconMime: primary.iconMime ?? fallback?.iconMime,
    gridMime: primary.gridMime ?? fallback?.gridMime,
    heroMime: primary.heroMime ?? fallback?.heroMime,
    logoMime: primary.logoMime ?? fallback?.logoMime,
  };
}

function artFromCache(cached: CoverRow): GameArt {
  return {
    icon: cached.icon ?? undefined,
    grid: cached.grid ?? undefined,
    hero: cached.hero ?? undefined,
    logo: cached.logo ?? undefined,
    iconMime: cached.icon_mime ?? undefined,
    gridMime: cached.grid_mime ?? undefined,
    heroMime: cached.hero_mime ?? undefined,
    logoMime: cached.logo_mime ?? undefined,
  };
}

function publicArt(art: GameArt): GameArt {
  return {
    icon: art.icon,
    grid: art.grid,
    hero: art.hero,
    logo: art.logo ? art.logo : undefined,
    iconMime: art.iconMime,
    gridMime: art.gridMime,
    heroMime: art.heroMime,
    logoMime: art.logoMime,
  };
}

async function resolveOne(apiKey: string, game: GameRef): Promise<GameArt | null> {
  const pathSuffix = await resolveSgdbPath(apiKey, game);
  if (!pathSuffix) {
    if (game.id.startsWith('steam:')) {
      const appId = game.id.slice('steam:'.length);
      return appId ? steamCdnArt(appId) : null;
    }
    return null;
  }
  const fromSgdb = await fetchArtBundle(apiKey, pathSuffix);
  if (game.id.startsWith('steam:')) {
    const appId = game.id.slice('steam:'.length);
    return mergeArt(fromSgdb, appId ? steamCdnArt(appId) : undefined);
  }
  if (!fromSgdb.icon && !fromSgdb.grid && !fromSgdb.hero && !fromSgdb.logo) {
    return null;
  }
  return fromSgdb;
}

async function resolveLogoOnly(
  apiKey: string,
  game: GameRef,
): Promise<{ url: string; mime: string }> {
  const pathSuffix = await resolveSgdbPath(apiKey, game);
  if (!pathSuffix) return { url: '', mime: '' };
  const asset = await firstAsset(
    apiKey,
    'logos',
    pathSuffix,
    '?types=static,animated',
  );
  return { url: asset?.url ?? '', mime: asset?.mime ?? '' };
}

/**
 * icon → Home rail, grid → Library tiles, hero → selected/featured, logo → home title.
 * @see https://www.steamgriddb.com/api/v2
 */
export async function resolveSteamCovers(
  games: GameRef[],
): Promise<Record<string, GameArt>> {
  getDb();
  const apiKey = loadSettings().steamGridDbApiKey?.trim();
  const out: Record<string, GameArt> = {};

  if (!apiKey) {
    for (const game of games) {
      if (!game.id.startsWith('steam:')) continue;
      const appId = game.id.slice('steam:'.length);
      if (appId) {
        const cached = getCover(game.id);
        if (cached) {
          out[game.id] = publicArt(
            mergeArt(artFromCache(cached), steamCdnArt(appId)),
          );
        } else {
          out[game.id] = publicArt(steamCdnArt(appId));
        }
      }
    }
    return out;
  }

  for (const game of games) {
    const cached = getCover(game.id);
    const hasArt =
      cached &&
      (cached.icon ||
        cached.grid ||
        cached.hero ||
        cached.logo != null ||
        cached.icon_manual ||
        cached.grid_manual ||
        cached.hero_manual ||
        cached.logo_manual);

    if (hasArt && cached) {
      if (cached.logo === null && !cached.logo_manual) {
        try {
          const logo = await resolveLogoOnly(apiKey, game);
          const merged = {
            ...artFromCache(cached),
            logo: logo.url,
            logoMime: logo.mime || undefined,
          };
          setCover(game.id, merged);
          out[game.id] = publicArt(merged);
        } catch {
          out[game.id] = publicArt(artFromCache(cached));
        }
        continue;
      }
      out[game.id] = publicArt(artFromCache(cached));
      continue;
    }

    try {
      const art = await resolveOne(apiKey, game);
      if (art) {
        setCover(game.id, art);
        out[game.id] = publicArt({
          ...artFromCache(getCover(game.id) ?? ({} as CoverRow)),
          ...art,
          logo: art.logo || undefined,
        });
        const after = getCover(game.id);
        if (after) out[game.id] = publicArt(artFromCache(after));
      }
    } catch {
      // skip
    }
  }

  return out;
}

export async function listSgdbAssets(
  game: GameRef,
  kind: ArtSlot,
): Promise<SgdbAsset[]> {
  const apiKey = loadSettings().steamGridDbApiKey?.trim();
  if (!apiKey) {
    throw new Error('SteamGridDB API key not configured');
  }
  const pathSuffix = await resolveSgdbPath(apiKey, game);
  if (!pathSuffix) return [];

  const apiKind = SLOT_TO_API[kind];
  const query =
    kind === 'grid'
      ? '?dimensions=600x900&types=static,animated'
      : '?types=static,animated';
  const res = await fetch(`${SGDB}/${apiKind}/${pathSuffix}${query}`, {
    headers: authHeaders(apiKey),
  });
  if (!res.ok) {
    throw new Error(`SteamGridDB ${apiKind} failed (${res.status})`);
  }
  const json = (await res.json()) as {
    data?: Array<{
      id?: number;
      url?: string;
      thumb?: string;
      mime?: string;
      style?: string;
      type?: string;
    }>;
  };
  const rows = Array.isArray(json.data) ? json.data : [];
  return rows
    .map((row) => {
      const url = row.url ?? row.thumb;
      if (!url || typeof row.id !== 'number') return null;
      const mime = row.mime ?? '';
      const animated = row.type === 'animated' || mime.toLowerCase().startsWith('video/');
      return {
        id: row.id,
        url,
        thumb: row.thumb ?? url,
        mime,
        style: row.style ?? '',
        animated,
      } satisfies SgdbAsset;
    })
    .filter((x): x is SgdbAsset => x != null);
}

export function applyCoverAsset(
  gameId: string,
  kind: ArtSlot,
  url: string,
  mime: string,
): GameArt {
  setCoverManual(gameId, kind, url, mime || null);
  const row = getCover(gameId);
  return row ? publicArt(artFromCache(row)) : {};
}

export async function resetCoverAsset(
  game: GameRef,
  kind: ArtSlot,
): Promise<GameArt> {
  clearCoverManual(game.id, kind);
  const apiKey = loadSettings().steamGridDbApiKey?.trim();
  if (apiKey) {
    try {
      const art = await resolveOne(apiKey, game);
      if (art) setCover(game.id, art);
    } catch {
      // keep cleared
    }
  } else if (game.id.startsWith('steam:')) {
    const appId = game.id.slice('steam:'.length);
    if (appId) setCover(game.id, steamCdnArt(appId));
  }
  const row = getCover(game.id);
  return row ? publicArt(artFromCache(row)) : {};
}
