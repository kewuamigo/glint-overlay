import fs from 'node:fs';
import path from 'node:path';
import { detectExe, detectScanDirs, resolveSteamAppId } from './detect.js';
import { ensureGseForGame } from './gse-cache.js';
import { resolveLibraryExePath, syncLibraryGame } from './schema-import.js';
import { fetchAndImportSteamAchievements } from './steam-fetch.js';
import { enableUpcForGame } from './upc-bridge.js';
import {
  getPrepareStatus,
  setPrepareStatus,
  type PrepareState,
  type PrepareStatus,
} from './store.js';

export type PrepareGameInput = {
  id: string;
  name: string;
  exe: string;
  install_path: string;
  source: string;
  steamApiKey?: string;
  /** Caller-supplied AppID (UI) when disk / library id have none. */
  steamAppId?: string;
  /** Bypass the official-Steam skip and install GSE anyway. */
  forceGse?: boolean;
};

/** True when Play must not spawn (pending, failed, or no status yet). */
export function prepareBlocksLaunch(status: PrepareState | null): boolean {
  if (!status) return true;
  return status.status === 'pending' || status.status === 'failed';
}

/** AppID from scanned Steam library id (`steam:3751950`). */
function steamAppIdFromLibraryId(id: string): string | null {
  const m = /^steam:(\d+)$/.exec(id);
  return m ? m[1] : null;
}

function steamAppIdFromCaller(raw: string | undefined): string | null {
  const t = raw?.trim() ?? '';
  return /^\d+$/.test(t) ? t : null;
}

function done(
  id: string,
  status: PrepareStatus,
  extra?: { error?: string; steamAppId?: string },
): PrepareState {
  setPrepareStatus(id, status, extra);
  return getPrepareStatus(id)!;
}

/** Detect → AppID → optional GSE → Steam/Epic schema → prepare status. */
export async function prepareGame(
  input: PrepareGameInput,
  opts?: { cacheDir?: string },
): Promise<PrepareState> {
  const { id } = input;
  try {
    setPrepareStatus(id, 'pending');
    const exePath = resolveLibraryExePath(input.exe, input.install_path);
    if (!exePath) {
      return done(id, 'failed', { error: 'Executable not found' });
    }

    const detect = detectExe(exePath);
    if (!input.forceGse && detect.platform === 'epic') {
      await syncLibraryGame({
        id: input.id,
        name: input.name,
        exe: input.exe,
        install_path: input.install_path,
      });
      return done(id, 'ready');
    }
    if (!input.forceGse && detect.platform !== 'steam') {
      return done(id, 'skipped');
    }

    const appId =
      resolveSteamAppId(exePath) ??
      steamAppIdFromLibraryId(input.id) ??
      steamAppIdFromCaller(input.steamAppId);
    if (!appId) {
      return done(id, 'failed', { error: 'Steam AppID is missing.' });
    }

    let schemaGameDir: string | undefined;
    const patchSource = input.forceGse ? 'custom' : input.source;
    if (patchSource !== 'steam') {
      const ensured = await ensureGseForGame({
        exePath,
        source: patchSource,
        appId,
        cacheDir: opts?.cacheDir,
        force: input.forceGse === true,
      });
      // Watchable GSE still needs the schema file: SetAchievement is a no-op
      // when defined_achievements is empty. Write next to existing steam_settings
      // (array format, no icon URLs — object maps crash GSE icon pagination).
      schemaGameDir =
        detectScanDirs(exePath).find((d) =>
          fs.existsSync(path.join(d, 'steam_settings')),
        ) ?? (ensured.installed ? ensured.gameDir : undefined);
    }

    await fetchAndImportSteamAchievements(
      id,
      appId,
      input.steamApiKey ?? '',
      schemaGameDir,
    );
    if (detect.markers.includes('uplay') && patchSource !== 'steam') {
      try {
        enableUpcForGame(exePath);
      } catch {
        // GSE + schema still count as ready; UPC loader swap needs a cache.
      }
    }
    return done(id, 'ready', { steamAppId: appId });
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    return done(id, 'failed', { error });
  }
}
