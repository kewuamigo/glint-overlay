import { gameRootFromExe } from '@glint/achievements-core';
import {
  getAllPlaytimeSeconds,
  getDb,
  insertCustomGame,
  listCustomGames,
  softDeleteAllCustomGames,
} from './db.js';

function exeBasename(exe: string): string {
  const parts = exe.replace(/[/\\]+$/, '').split(/[/\\]/);
  return parts[parts.length - 1] || exe;
}

export type CustomGame = {
  name: string;
  executable: string;
};

/** Prefer install folder title over CamelCase exe stem (AlanWake2 → Alan Wake 2). */
export function titleFromExePath(exePath: string): string {
  const parts = exePath.replace(/[/\\]+$/, '').split(/[/\\]/).filter(Boolean);
  const file = parts.pop() ?? 'game.exe';
  const stem = file.replace(/\.exe$/i, '') || file;
  const folder = parts[parts.length - 1] ?? '';
  if (
    folder &&
    !/^(bin|binaries|win32|win64|x64|x86|game|games|shipping)$/i.test(folder)
  ) {
    if (/[\s_]/.test(folder) || folder.length >= stem.length) {
      return folder.replace(/_/g, ' ').trim();
    }
  }
  return stem
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
    .replace(/([A-Za-z])(\d)/g, '$1 $2')
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export function loadCustomGames(): CustomGame[] {
  getDb();
  return listCustomGames().map((g) => ({
    name: titleFromExePath(g.executable),
    executable: g.executable,
  }));
}

export function clearCustomGames(): void {
  getDb();
  softDeleteAllCustomGames();
}

export function addCustomGame(name: string, executable: string): CustomGame {
  const trimmedExe = executable.trim();
  if (!trimmedExe) {
    throw new Error('executable is required');
  }
  const trimmedName = name.trim() || titleFromExePath(trimmedExe);
  getDb();
  const entry = insertCustomGame(trimmedName, trimmedExe);
  return { name: entry.name, executable: entry.executable };
}

export type ScannedGame = {
  id: string;
  name: string;
  source: string;
  exe: string;
  install_path: string;
  running: boolean;
  pid?: number;
  playtime_hours: number | null;
};

function attachPlaytime(games: ScannedGame[]): ScannedGame[] {
  const totals = getAllPlaytimeSeconds();
  return games.map((game) => {
    const seconds = totals[game.id] ?? 0;
    return {
      ...game,
      playtime_hours: seconds > 0 ? Math.round((seconds / 3600) * 10) / 10 : null,
    };
  });
}

export function withPlaytime(games: ScannedGame[]): ScannedGame[] {
  getDb();
  return attachPlaytime(games);
}

export function applyRunningState(
  games: ScannedGame[],
  processes: { pid: number; name: string }[],
): ScannedGame[] {
  return games.map((game) => {
    const base = exeBasename(game.exe).toLowerCase();
    const proc = processes.find((p) => p.name.toLowerCase() === base);
    if (proc) {
      return { ...game, running: true, pid: proc.pid };
    }
    return { ...game, running: false, pid: undefined };
  });
}

export function mergeCustomGames(
  games: ScannedGame[],
  processes: { pid: number; name: string }[],
): ScannedGame[] {
  getDb();
  const out = [...games];
  for (const custom of loadCustomGames()) {
    const customBase = exeBasename(custom.executable).toLowerCase();
    if (out.some((g) => exeBasename(g.exe).toLowerCase() === customBase)) {
      continue;
    }
    const hasPath = /[/\\]/.test(custom.executable);
    out.push({
      id: `custom:${custom.executable}`,
      name: custom.name,
      source: 'custom',
      exe: custom.executable,
      install_path: hasPath ? gameRootFromExe(custom.executable) : '',
      running: false,
      playtime_hours: null,
    });
  }
  return attachPlaytime(applyRunningState(out, processes));
}
