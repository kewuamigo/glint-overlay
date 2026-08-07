import { getCover, getDb, setCover } from './db.js';
import { loadSettings } from './settings.js';

export type GameArt = {
  icon?: string;
  grid?: string;
  hero?: string;
};

type GameRef = {
  id: string;
  name: string;
};

type AssetKind = 'grids' | 'icons' | 'heroes';

const SGDB = 'https://www.steamgriddb.com/api/v2';

function steamCdnArt(appId: string): GameArt {
  const base = `https://cdn.cloudflare.steamstatic.com/steam/apps/${appId}`;
  return {
    icon: `${base}/capsule_231x87.jpg`,
    grid: `${base}/library_600x900.jpg`,
    hero: `${base}/library_hero.jpg`,
  };
}

function authHeaders(apiKey: string): HeadersInit {
  return { Authorization: `Bearer ${apiKey}` };
}

async function firstAssetUrl(
  apiKey: string,
  kind: AssetKind,
  pathSuffix: string,
  query = '',
): Promise<string | null> {
  const res = await fetch(`${SGDB}/${kind}/${pathSuffix}${query}`, {
    headers: authHeaders(apiKey),
  });
  if (!res.ok) return null;
  const json = (await res.json()) as {
    data?: Array<{ url?: string; thumb?: string }>;
  };
  const first = json.data?.[0];
  return first?.url ?? first?.thumb ?? null;
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

async function fetchArtBundle(
  apiKey: string,
  pathSuffix: string,
  gridQuery: string,
): Promise<GameArt> {
  const [icon, grid, hero] = await Promise.all([
    firstAssetUrl(apiKey, 'icons', pathSuffix),
    firstAssetUrl(apiKey, 'grids', pathSuffix, gridQuery),
    firstAssetUrl(apiKey, 'heroes', pathSuffix),
  ]);
  return {
    icon: icon ?? undefined,
    grid: grid ?? undefined,
    hero: hero ?? undefined,
  };
}

function mergeArt(primary: GameArt, fallback?: GameArt): GameArt {
  return {
    icon: primary.icon ?? fallback?.icon,
    grid: primary.grid ?? fallback?.grid,
    hero: primary.hero ?? fallback?.hero,
  };
}

async function resolveOne(apiKey: string, game: GameRef): Promise<GameArt | null> {
  if (game.id.startsWith('steam:')) {
    const appId = game.id.slice('steam:'.length);
    if (!appId) return null;
    const fromSgdb = await fetchArtBundle(
      apiKey,
      `steam/${encodeURIComponent(appId)}`,
      '?dimensions=600x900',
    );
    return mergeArt(fromSgdb, steamCdnArt(appId));
  }

  const sgdbId = await searchGameIdByName(apiKey, game.name);
  if (sgdbId == null) return null;
  const art = await fetchArtBundle(apiKey, `game/${sgdbId}`, '?dimensions=600x900');
  if (!art.icon && !art.grid && !art.hero) return null;
  return art;
}

/**
 * icon → Home, grid → Library tiles, hero → selected/featured.
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
      if (appId) out[game.id] = steamCdnArt(appId);
    }
    return out;
  }

  for (const game of games) {
    const cached = getCover(game.id);
    if (cached && (cached.icon || cached.grid || cached.hero)) {
      out[game.id] = {
        icon: cached.icon ?? undefined,
        grid: cached.grid ?? undefined,
        hero: cached.hero ?? undefined,
      };
      continue;
    }

    try {
      const art = await resolveOne(apiKey, game);
      if (art) {
        setCover(game.id, art);
        out[game.id] = art;
      }
    } catch {
      // skip
    }
  }

  return out;
}
