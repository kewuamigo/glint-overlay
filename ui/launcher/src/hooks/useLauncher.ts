import { useCallback, useEffect, useRef, useState } from 'react';
import {
  launcherInvoke,
  type InstalledApp,
  type Profile,
  type ScannedGame,
  type StoreCatalog,
  type GameArt,
  type PrepareState,
} from '../launcher-bridge';

export function useLauncher() {
  const [games, setGames] = useState<ScannedGame[]>([]);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [catalog, setCatalog] = useState<StoreCatalog | null>(null);
  const [installed, setInstalled] = useState<InstalledApp[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [launching, setLaunching] = useState<string | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [steamKeyConfigured, setSteamKeyConfigured] = useState(false);
  const [covers, setCovers] = useState<Record<string, GameArt>>({});
  const [coversTick, setCoversTick] = useState(0);
  const [prepareStatuses, setPrepareStatuses] = useState<
    Record<string, PrepareState>
  >({});
  const [forcingGse, setForcingGse] = useState<Record<string, true>>({});
  const prepareInFlight = useRef(new Set<string>());

  const refreshPrepareStatuses = useCallback(async () => {
    try {
      const map = await launcherInvoke<Record<string, PrepareState>>(
        'achievements.prepareStatus',
      );
      if (!map || typeof map !== 'object') return;
      setPrepareStatuses((prev) => {
        const next = { ...map };
        for (const id of prepareInFlight.current) {
          if (prev[id]?.status === 'pending') {
            next[id] = prev[id];
          }
        }
        return next;
      });
    } catch {
      /* ignore */
    }
  }, []);

  const refreshGames = useCallback(async (fullScan = false) => {
    try {
      const res = await launcherInvoke<{ games: ScannedGame[] }>(
        'games.scan',
        fullScan ? [true] : [],
      );
      setGames(res.games ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
    await refreshPrepareStatuses();
  }, [refreshPrepareStatuses]);

  const retryPrepare = useCallback(
    async (game: ScannedGame, steamAppId?: string, forceGse?: boolean) => {
      if (forceGse) {
        setForcingGse((prev) => ({ ...prev, [game.id]: true }));
      }
      prepareInFlight.current.add(game.id);
      setPrepareStatuses((prev) => ({
        ...prev,
        [game.id]: {
          status: 'pending',
          steamAppId,
          updatedAt: Date.now(),
        },
      }));
      try {
        await launcherInvoke('achievements.prepare', [
          {
            ...game,
            ...(steamAppId ? { steamAppId } : {}),
            ...(forceGse ? { forceGse: true } : {}),
          },
        ]);
        prepareInFlight.current.delete(game.id);
        await refreshPrepareStatuses();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        prepareInFlight.current.delete(game.id);
        await refreshPrepareStatuses();
      } finally {
        prepareInFlight.current.delete(game.id);
        setForcingGse((prev) => {
          if (!prev[game.id]) return prev;
          const next = { ...prev };
          delete next[game.id];
          return next;
        });
      }
    },
    [refreshPrepareStatuses],
  );

  const coversKey = games.map((g) => `${g.id}\t${g.name}`).join('\n');

  useEffect(() => {
    if (!coversKey) return;
    const refs = coversKey.split('\n').map((line) => {
      const tab = line.indexOf('\t');
      return {
        id: tab === -1 ? line : line.slice(0, tab),
        name: tab === -1 ? line : line.slice(tab + 1),
      };
    });
    let cancelled = false;
    void launcherInvoke<Record<string, GameArt>>('covers.resolve', [refs])
      .then((map) => {
        if (!cancelled) setCovers(map);
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [coversKey, coversTick]);

  const refreshInstalled = useCallback(async () => {
    try {
      const apps = await launcherInvoke<InstalledApp[]>('apps.installed');
      setInstalled(Array.isArray(apps) ? apps : []);
    } catch {
      setInstalled([]);
    }
  }, []);

  const refreshCatalog = useCallback(async () => {
    try {
      const cat = await launcherInvoke<StoreCatalog>('store.catalog');
      setCatalog(cat);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const refreshAll = useCallback(async () => {
    setLoading(true);
    await Promise.all([
      refreshGames(true),
      refreshInstalled(),
      refreshCatalog(),
      launcherInvoke<Profile>('profile.get').then(setProfile).catch(() => null),
      launcherInvoke<{ steamApiKey: string }>('settings.get')
        .then((s) => setSteamKeyConfigured(Boolean(s.steamApiKey?.trim())))
        .catch(() => setSteamKeyConfigured(false)),
    ]);
    setLoading(false);
  }, [refreshGames, refreshInstalled, refreshCatalog]);

  useEffect(() => {
    void refreshAll();
    const timer = setInterval(() => void refreshGames(false), 2_000);
    const onVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshGames(false);
      }
    };
    window.addEventListener('focus', onVisible);
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      clearInterval(timer);
      window.removeEventListener('focus', onVisible);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [refreshAll, refreshGames]);

  const launchOverlay = useCallback(async (pid: number, game?: ScannedGame) => {
    setLaunching(String(pid));
    try {
      await launcherInvoke(
        'games.launchOverlay',
        game ? [pid, game] : [pid],
      );
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLaunching(null);
    }
  }, []);

  const launchGame = useCallback(
    async (game: ScannedGame) => {
      setLaunching(game.id);
      try {
        await launcherInvoke('games.launch', [game]);
        await refreshGames(false);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLaunching(null);
      }
    },
    [refreshGames],
  );

  const installApp = useCallback(
    async (id: string) => {
      setInstalling(id);
      try {
        await launcherInvoke('store.install', [id]);
        await refreshInstalled();
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setInstalling(null);
      }
    },
    [refreshInstalled],
  );

  const addGame = useCallback(
    async (name: string, executable: string) => {
      try {
        const entry = await launcherInvoke<{ name: string; executable: string }>(
          'games.add',
          [name, executable],
        );
        await refreshGames(true);
        await launcherInvoke('achievements.syncFromLibrary', [
          [
            {
              id: `custom:${entry.executable}`,
              name: entry.name,
              exe: entry.executable,
              install_path: '',
            },
          ],
        ]);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [refreshGames],
  );

  const resetLibrary = useCallback(async () => {
    try {
      const res = await launcherInvoke<{ games: ScannedGame[] }>('games.reset');
      setGames(res.games ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const runningGames = games.filter((g) => g.running);
  const featured =
    runningGames[0] ??
    games.find((g) => g.source === 'steam') ??
    games[0] ??
    null;

  return {
    games,
    covers,
    refreshCovers: () => setCoversTick((n) => n + 1),
    patchCover: (gameId: string, art: GameArt) => {
      setCovers((prev) => ({
        ...prev,
        [gameId]: { ...prev[gameId], ...art },
      }));
    },
    profile,
    catalog,
    installed,
    loading,
    error,
    launching,
    installing,
    featured,
    prepareStatuses,
    forcingGse,
    steamKeyConfigured,
    setSteamKeyConfigured,
    refreshAll,
    refreshGames,
    refreshCatalog,
    launchOverlay,
    launchGame,
    retryPrepare,
    installApp,
    addGame,
    resetLibrary,
  };
}
