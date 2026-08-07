import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import type { BrowserWindow } from 'electron';
import { dialog, ipcMain, shell } from 'electron';
import {
  detectExe,
  getEmuGuide,
  listAchievementsForGame,
  listTrackedGames,
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
import { resolveSteamCovers } from './steamgriddb.js';
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

async function waitForNewProcessPid(
  exePath: string,
  existingPids: Set<number>,
  spawnedPid: number | undefined,
  timeoutMs = 90_000,
): Promise<number | null> {
  const base = path.basename(exePath).toLowerCase();
  const deadline = Date.now() + timeoutMs;
  let lastPid: number | null = null;
  let stable = 0;
  // Require the same PID across several polls so bootstrap stubs that exit
  // immediately are not injected.
  const needStable = 3;
  while (Date.now() < deadline) {
    const procs = rustCli<{ processes: { pid: number; name: string }[] }>(
      'processes',
    );
    const hit =
      (spawnedPid &&
        !existingPids.has(spawnedPid) &&
        procs.processes?.find((p) => p.pid === spawnedPid)) ||
      procs.processes?.find(
        (p) => p.name.toLowerCase() === base && !existingPids.has(p.pid),
      );
    if (hit?.pid) {
      if (hit.pid === lastPid) {
        stable += 1;
        if (stable >= needStable) return hit.pid;
      } else {
        lastPid = hit.pid;
        stable = 1;
      }
    } else {
      lastPid = null;
      stable = 0;
    }
    await new Promise((r) => setTimeout(r, 750));
  }
  return lastPid;
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

  const exeBase = path.basename(exePath).toLowerCase();
  const beforeProcs = rustCli<{ processes: { pid: number; name: string }[] }>(
    'processes',
  );
  const existingPids = new Set(
    (beforeProcs.processes ?? [])
      .filter((p) => p.name.toLowerCase() === exeBase)
      .map((p) => p.pid),
  );

  const child = spawn(exePath, args, {
    cwd,
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  });
  const spawnedPid = child.pid;
  child.unref();

  if (skipOverlay) return { ok: true, pid: spawnedPid };

  const pid = await waitForNewProcessPid(exePath, existingPids, spawnedPid);
  if (pid) {
    syncOverlayDebugFlag(loadSettings().overlayDebug);
    await rustCliAsync('inject', [String(pid), path.basename(exePath)]);
    return { ok: true, pid };
  }
  return { ok: true, pid: spawnedPid };
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
        });
        syncOverlayDebugFlag(next.overlayDebug);
        return next;
      }
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
        return { games: withPlaytime(merged) };
      }
      case 'games.add': {
        const name = String(args[0] ?? '');
        const executable = String(args[1] ?? '');
        const entry = addCustomGame(name, executable);
        cachedGames = null;
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
