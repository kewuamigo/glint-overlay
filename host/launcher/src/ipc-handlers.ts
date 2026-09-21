import fs from 'node:fs';
import path from 'node:path';
import type { BrowserWindow } from 'electron';
import { app, dialog, ipcMain, shell } from 'electron';
import {
  detectExe,
  findGameExeInFolder,
  gameRootFromExe,
  getEmuGuide,
  getPrepareStatus,
  listAchievementsForGame,
  listPrepareStatuses,
  listTrackedGames,
  prepareBlocksLaunch,
  prepareGame,
  resolveEpicNamespace,
  resolveLibraryExePath,
  syncLibraryGame,
  trackGameFromExe,
} from '@glint/achievements-core';
import { listInstalledApps } from './apps-scan.js';
import { loadCatalog } from './catalog.js';
import { addCustomGame, clearCustomGames, mergeCustomGames, withPlaytime, type ScannedGame } from './custom-games.js';
import { getDb } from './db.js';
import { tickPlaytime } from './playtime.js';
import { rustCli, rustCliAsync } from './rust-cli.js';
import { installStoreApp } from './store-install.js';
import { loadSettings, saveSettings } from './settings.js';
import {
  overlayLogsDir,
  syncOverlayDebugFlag,
} from './overlay-debug.js';
import { steamNearestPing } from './steam-ping.js';
import { resolveSteamCovers, listSgdbAssets, applyCoverAsset, resetCoverAsset } from './steamgriddb.js';
import type { ArtSlot } from './db.js';
import {
  loadCloudStorageConfig,
  saveCloudStorageConfig,
  setCloudSyncEnabled,
  testCloudStorageConnection,
} from './cloud-storage-config.js';
import {
  catchUpBeforeLaunch,
  deleteCloudRevision,
  getCloudSyncStatus,
  listCloudRevisions,
  resolveTrackedGameFromPid,
  restoreCloudRevision,
  runSync,
  setCloudSyncStatusListener,
  toSyncGameRef,
  trackGameProcess,
} from './cloud-sync-engine.js';
import type { CloudStorageConfig } from '@glint/cloud-sync';
import {
  applyUpdate,
  checkForUpdates,
  configureOta,
  dismissUpdate,
  getOtaStatus,
  openReleasePage,
} from './ota.js';

function epicEmuArgs(namespace: string): string[] {
  // No AUTH_/EpicPortal — those GPU-crash AW2. Identity matches nepice_settings.
  return [
    `-epicapp=${namespace}`,
    `-epicsandboxid=${namespace}`,
    '-epicenv=Prod',
    '-epicusername=DefaultName',
    '-epicuserid=69bfe74044409f2feb6f5e11c695a47a',
    '-epiclocale=en',
  ];
}

async function launchScannedGame(game: ScannedGame): Promise<{
  ok: boolean;
  cancelled?: boolean;
  pid?: number;
}> {
  const exePath = resolveLibraryExePath(game.exe, game.install_path ?? '');
  if (!exePath) throw new Error(`executable not found: ${game.exe}`);
  const cwd = path.dirname(exePath);
  const detect = detectExe(exePath);
  let args: string[] = [];

  let skipOverlay = false;
  if (detect.offerEpicEmuLaunch) {
    const boxOpts = {
      type: 'question' as const,
      buttons: [
        'Launch with EpicEmu',
        'EpicEmu (no overlay)',
        'Launch normally',
        'Cancel',
      ],
      defaultId: 0,
      cancelId: 3,
      title: 'Epic emulator',
      message: `${game.name} looks like a Nemirtingas Epic title.`,
      detail:
        'EpicEmu (no overlay): same auth args, no inject — use this to accept EULA first.',
    };
    const { response } = mainWindow
      ? await dialog.showMessageBox(mainWindow, boxOpts)
      : await dialog.showMessageBox(boxOpts);
    if (response === 3) return { ok: false, cancelled: true };
    if (response === 0 || response === 1) {
      const ns = await resolveEpicNamespace(game.id, game.name, cwd);
      if (!ns) {
        throw new Error(
          'Could not resolve Epic namespace (egdata / store slug). Sync achievements once first.',
        );
      }
      args = epicEmuArgs(ns);
      if (response === 1) skipOverlay = true;
    }
  }

  if (!skipOverlay) {
    syncOverlayDebugFlag(loadSettings().overlayDebug);
  }
  const extra = [exePath, '--cwd', cwd];
  if (skipOverlay) extra.push('--skip-overlay');
  if (args.length) extra.push('--', ...args);
  const result = await rustCliAsync<{ ok: boolean; pid?: number }>(
    'launch',
    extra,
  );
  return { ok: result.ok, pid: result.pid };
}

export type LauncherInvokeRequest = {
  method: string;
  args?: unknown[];
};

let mainWindow: BrowserWindow | null = null;
let cachedGames: ScannedGame[] | null = null;

export function setLauncherWindow(win: BrowserWindow | null): void {
  mainWindow = win;
  setCloudSyncStatusListener(
    win
      ? (payload) => {
          try {
            win.webContents.send('cloud.sync.status', payload);
          } catch {
            // window gone
          }
        }
      : null,
  );
  configureOta({
    quit: () => {
      app.quit();
    },
    push: (payload) => {
      if (!win) return;
      try {
        win.webContents.send('ota.status', payload);
      } catch {
        // window gone
      }
    },
  });
}

function kickPrepare(game: {
  id?: string;
  name?: string;
  exe?: string;
  install_path?: string;
  source?: string;
  steamAppId?: string;
}): void {
  if (!game.id || !game.name || !game.exe) return;
  void prepareGame({
    id: game.id,
    name: game.name,
    exe: game.exe,
    install_path: game.install_path ?? '',
    source: game.source ?? 'custom',
    steamApiKey: loadSettings().steamApiKey,
    steamAppId: game.steamAppId,
  });
}

export function registerLauncherIpc(): void {
  getDb();
  ipcMain.handle('launcher:invoke', async (_event, req: LauncherInvokeRequest) => {
    const args = req.args ?? [];
    switch (req.method) {
      case 'profile.get':
        return rustCli('profile');
      case 'system.ping':
        return steamNearestPing();
      case 'settings.get':
        return loadSettings();
      case 'settings.set': {
        const patch = (args[0] ?? {}) as {
          steamApiKey?: string;
          steamGridDbApiKey?: string;
          overlayDebug?: boolean;
          otaAutoCheck?: boolean;
        };
        const next = saveSettings({
          steamApiKey:
            typeof patch.steamApiKey === 'string' ? patch.steamApiKey.trim() : undefined,
          steamGridDbApiKey:
            typeof patch.steamGridDbApiKey === 'string'
              ? patch.steamGridDbApiKey.trim()
              : undefined,
          overlayDebug:
            typeof patch.overlayDebug === 'boolean' ? patch.overlayDebug : undefined,
          otaAutoCheck:
            typeof patch.otaAutoCheck === 'boolean' ? patch.otaAutoCheck : undefined,
        });
        syncOverlayDebugFlag(next.overlayDebug);
        return next;
      }
      case 'ota.getStatus':
        return getOtaStatus();
      case 'ota.check':
        return checkForUpdates(true);
      case 'ota.apply':
        return applyUpdate();
      case 'ota.openRelease':
        return openReleasePage();
      case 'ota.dismiss':
        return dismissUpdate();
      case 'cloud.storage.getConfig':
        return loadCloudStorageConfig();
      case 'cloud.storage.setConfig': {
        const raw = args[0];
        if (raw === null || raw === undefined) {
          return saveCloudStorageConfig(null);
        }
        const cfg = raw as CloudStorageConfig;
        if (!cfg || typeof cfg !== 'object' || typeof cfg.provider !== 'string') {
          throw new Error('cloud.storage.setConfig requires a CloudStorageConfig or null');
        }
        return saveCloudStorageConfig(cfg);
      }
      case 'cloud.storage.testConnection': {
        const override =
          args[0] === undefined
            ? undefined
            : args[0] === null
              ? null
              : (args[0] as CloudStorageConfig);
        return testCloudStorageConnection(override);
      }
      case 'cloud.sync.setEnabled': {
        const enabled = args[0] === true;
        return setCloudSyncEnabled(enabled);
      }
      case 'cloud.sync.getStatus': {
        const gameId =
          typeof args[0] === 'string'
            ? args[0]
            : args[0] && typeof args[0] === 'object'
              ? String((args[0] as { gameId?: string; id?: string }).gameId ??
                  (args[0] as { id?: string }).id ??
                  '')
              : undefined;
        return getCloudSyncStatus(gameId || undefined);
      }
      case 'cloud.sync.now': {
        const raw = (args[0] ?? {}) as Partial<ScannedGame> & {
          gameId?: string;
        };
        const opts = (args[1] ?? {}) as {
          overwrite?: boolean;
          keepRemote?: boolean;
        };
        const game = toSyncGameRef({
          id: raw.id ?? raw.gameId,
          name: raw.name,
          exe: raw.exe,
          install_path: raw.install_path,
        });
        return runSync(game, {
          overwrite: opts.overwrite === true,
          keepRemote: opts.keepRemote === true,
          unattended: false,
        });
      }
      case 'cloud.sync.listRevisions': {
        const raw = args[0];
        const gameId =
          typeof raw === 'string'
            ? raw
            : String(
                (raw as { gameId?: string; id?: string } | undefined)?.gameId ??
                  (raw as { id?: string } | undefined)?.id ??
                  '',
              );
        if (!gameId) throw new Error('cloud.sync.listRevisions requires gameId');
        return listCloudRevisions(gameId);
      }
      case 'cloud.sync.restore': {
        const raw = (args[0] ?? {}) as Partial<ScannedGame> & {
          gameId?: string;
          revisionId?: string;
        };
        const revisionId =
          typeof args[1] === 'string'
            ? args[1]
            : typeof raw.revisionId === 'string'
              ? raw.revisionId
              : undefined;
        const game = toSyncGameRef({
          id: raw.id ?? raw.gameId,
          name: raw.name,
          exe: raw.exe,
          install_path: raw.install_path,
        });
        return restoreCloudRevision(game, revisionId);
      }
      case 'cloud.sync.deleteRevision': {
        const raw = args[0];
        const gameId =
          typeof raw === 'string'
            ? raw
            : String(
                (raw as { gameId?: string; id?: string } | undefined)?.gameId ??
                  (raw as { id?: string } | undefined)?.id ??
                  '',
              );
        const revisionId =
          typeof args[1] === 'string'
            ? args[1]
            : typeof raw === 'object' && raw && 'revisionId' in raw
              ? String((raw as { revisionId?: string }).revisionId ?? '')
              : '';
        if (!gameId) {
          throw new Error('cloud.sync.deleteRevision requires gameId');
        }
        if (!revisionId) {
          throw new Error('cloud.sync.deleteRevision requires revisionId');
        }
        return deleteCloudRevision(gameId, revisionId);
      }
      case 'logs.openDir': {
        const dir = overlayLogsDir();
        fs.mkdirSync(dir, { recursive: true });
        await shell.openPath(dir);
        return { ok: true, dir };
      }
      case 'covers.resolve': {
        const raw = Array.isArray(args[0]) ? args[0] : [];
        const games = raw.map((item) => {
          if (typeof item === 'string') {
            return { id: item, name: item };
          }
          const obj = item as { id?: string; name?: string };
          return {
            id: String(obj.id ?? ''),
            name: String(obj.name ?? ''),
          };
        });
        return resolveSteamCovers(games);
      }
      case 'covers.listAssets': {
        const gameId = String(args[0] ?? '');
        const name = String(args[1] ?? '');
        const kind = String(args[2] ?? '') as ArtSlot;
        if (!gameId || !['icon', 'grid', 'hero', 'logo'].includes(kind)) {
          throw new Error('covers.listAssets requires gameId and kind');
        }
        return listSgdbAssets({ id: gameId, name: name || gameId }, kind);
      }
      case 'covers.setAsset': {
        const gameId = String(args[0] ?? '');
        const kind = String(args[1] ?? '') as ArtSlot;
        const url = String(args[2] ?? '');
        const mime = String(args[3] ?? '');
        if (!gameId || !url || !['icon', 'grid', 'hero', 'logo'].includes(kind)) {
          throw new Error('covers.setAsset requires gameId, kind, url');
        }
        return applyCoverAsset(gameId, kind, url, mime);
      }
      case 'covers.clearAsset': {
        const gameId = String(args[0] ?? '');
        const name = String(args[1] ?? '');
        const kind = String(args[2] ?? '') as ArtSlot;
        if (!gameId || !['icon', 'grid', 'hero', 'logo'].includes(kind)) {
          throw new Error('covers.clearAsset requires gameId and kind');
        }
        return resetCoverAsset({ id: gameId, name: name || gameId }, kind);
      }
      case 'processes.list':
        return rustCli('processes');
      case 'games.scan': {
        const fullScan = args[0] === true || !cachedGames;
        if (fullScan) {
          const res = rustCli<{ games: ScannedGame[] }>('scan-games');
          cachedGames = res.games ?? [];
        }
        const procs = rustCli<{ processes: { pid: number; name: string }[] }>(
          'processes',
        );
        const merged = mergeCustomGames(cachedGames ?? [], procs.processes ?? []);
        tickPlaytime(merged);
        for (const g of merged) {
          if (!getPrepareStatus(g.id)) kickPrepare(g);
        }
        return { games: withPlaytime(merged) };
      }
      case 'games.add': {
        const name = String(args[0] ?? '');
        const executable = String(args[1] ?? '');
        const entry = addCustomGame(name, executable);
        cachedGames = null;
        const exe = entry.executable;
        kickPrepare({
          id: `custom:${exe}`,
          name: entry.name,
          exe,
          install_path: /[/\\]/.test(exe) ? gameRootFromExe(exe) : '',
          source: 'custom',
        });
        return entry;
      }
      case 'games.reset': {
        clearCustomGames();
        cachedGames = null;
        const res = rustCli<{ games: ScannedGame[] }>('scan-games');
        cachedGames = res.games ?? [];
        const procs = rustCli<{ processes: { pid: number; name: string }[] }>(
          'processes',
        );
        const merged = mergeCustomGames(cachedGames, procs.processes ?? []);
        tickPlaytime(merged);
        return { games: withPlaytime(merged) };
      }
      case 'games.launchOverlay': {
        const pid = Number(args[0]);
        if (!pid || Number.isNaN(pid)) {
          throw new Error('games.launchOverlay requires numeric pid');
        }
        syncOverlayDebugFlag(loadSettings().overlayDebug);
        const result = await rustCliAsync('inject', [String(pid)]);
        const rawGame = args[1] as Partial<ScannedGame> | undefined;
        const gameRef = rawGame?.id
          ? toSyncGameRef({
              id: rawGame.id,
              name: rawGame.name,
              exe: rawGame.exe,
              install_path: rawGame.install_path,
            })
          : resolveTrackedGameFromPid(pid, cachedGames ?? []);
        if (gameRef) trackGameProcess(pid, gameRef);
        return result;
      }
      case 'games.launch': {
        const raw = (args[0] ?? {}) as Partial<ScannedGame>;
        if (!raw.exe && !raw.id) {
          throw new Error('games.launch requires a game');
        }
        const game: ScannedGame = {
          id: String(raw.id ?? `launch:${raw.exe}`),
          name: String(raw.name ?? path.basename(String(raw.exe ?? 'game'))),
          source: String(raw.source ?? 'custom'),
          exe: String(raw.exe ?? ''),
          install_path: String(raw.install_path ?? ''),
          running: false,
          playtime_hours: null,
        };
        const prepare = getPrepareStatus(game.id);
        if (!prepare) kickPrepare(game);
        if (prepareBlocksLaunch(prepare)) {
          throw new Error(
            prepare?.status === 'failed'
              ? (prepare.error ?? 'Achievement integration failed.')
              : 'Creating achievement integration…',
          );
        }
        // Catch-up sync before launch when local is dirty (no conflict pending).
        try {
          await catchUpBeforeLaunch({
            id: game.id,
            name: game.name,
            exe: game.exe,
            install_path: game.install_path,
          });
        } catch {
          // Don't block launch on catch-up failure; state records lastError.
        }
        const launched = await launchScannedGame(game);
        if (launched.pid) {
          trackGameProcess(launched.pid, {
            id: game.id,
            name: game.name,
            exe: game.exe,
            install_path: game.install_path,
          });
        }
        return launched;
      }
      case 'achievements.prepare': {
        const raw = (args[0] ?? {}) as {
          id?: string;
          name?: string;
          exe?: string;
          install_path?: string;
          source?: string;
          steamAppId?: string;
          forceGse?: boolean;
        };
        if (!raw.id || !raw.name || !raw.exe) return null;
        return prepareGame({
          id: raw.id,
          name: raw.name,
          exe: raw.exe,
          install_path: raw.install_path ?? '',
          source: raw.source ?? 'custom',
          steamApiKey: loadSettings().steamApiKey,
          steamAppId: raw.steamAppId,
          forceGse: raw.forceGse === true,
        });
      }
      case 'achievements.prepareStatus': {
        const id = args[0] != null && String(args[0]) !== '' ? String(args[0]) : '';
        return id ? getPrepareStatus(id) : listPrepareStatuses();
      }
      case 'achievements.listGames':
        return listTrackedGames();
      case 'achievements.listForGame':
        return listAchievementsForGame(String(args[0] ?? ''));
      case 'achievements.syncFromLibrary': {
        const games = Array.isArray(args[0]) ? args[0] : [];
        let lastGuide = getEmuGuide('unknown');
        for (const raw of games) {
          const g = raw as {
            id?: string;
            name?: string;
            exe?: string;
            install_path?: string;
          };
          if (!g.id || !g.name || !g.exe) continue;
          const detect = await syncLibraryGame({
            id: g.id,
            name: g.name,
            exe: g.exe,
            install_path: g.install_path ?? '',
          });
          if (detect) lastGuide = detect.guide;
        }
        return { games: listTrackedGames(), guide: lastGuide };
      }
      case 'achievements.detectExe':
        return detectExe(String(args[0] ?? ''));
      case 'achievements.trackExe':
        return trackGameFromExe(String(args[0] ?? ''));
      case 'achievements.getGuide':
        return getEmuGuide(String(args[0] ?? 'unknown'));
      case 'achievements.pickExe': {
        const result = await dialog.showOpenDialog({
          properties: ['openFile'],
          filters: [{ name: 'Executable', extensions: ['exe'] }],
        });
        if (result.canceled || !result.filePaths[0]) return null;
        return result.filePaths[0];
      }
      case 'games.pickFolder': {
        const result = await dialog.showOpenDialog({
          properties: ['openDirectory'],
        });
        if (result.canceled || !result.filePaths[0]) return null;
        const root = result.filePaths[0];
        const exe = findGameExeInFolder(root);
        if (!exe) {
          throw new Error('No game executable found in that folder');
        }
        return { exe, install_path: root, name: path.basename(root) };
      }
      case 'achievements.openUrl':
        await shell.openExternal(String(args[0] ?? ''));
        return undefined;
      case 'apps.installed':
        return listInstalledApps();
      case 'store.catalog':
        return loadCatalog();
      case 'store.install': {
        const id = String(args[0] ?? '');
        if (!id) throw new Error('store.install requires app id');
        await installStoreApp(id);
        return { ok: true, id };
      }
      case 'window.minimize':
        mainWindow?.minimize();
        return undefined;
      case 'window.maximize': {
        if (mainWindow?.isMaximized()) {
          mainWindow.unmaximize();
        } else {
          mainWindow?.maximize();
        }
        return undefined;
      }
      case 'window.close':
        mainWindow?.close();
        return undefined;
      default:
        throw new Error(`unknown launcher method: ${req.method}`);
    }
  });
}
