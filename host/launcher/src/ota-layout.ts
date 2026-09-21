import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { INNO_APP_ID } from './product.js';

export type InstallLayout = 'inno' | 'portable';

const INNO_MARKERS = ['unins000.exe', 'unins000.dat', 'Uninstall.exe'];

/** Inno writes `{AppId}_is1` under Uninstall (AppId without doubled braces). */
export function innoUninstallKey(): string {
  return `${INNO_APP_ID}_is1`;
}

export function hasInnoMarkers(installRoot: string): boolean {
  for (const name of INNO_MARKERS) {
    if (fs.existsSync(path.join(installRoot, name))) return true;
  }
  return false;
}

function uninstallHives(): string[] {
  const key = innoUninstallKey();
  return [
    `HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${key}`,
    `HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${key}`,
    `HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${key}`,
  ];
}

function parseInstallLocation(regOutput: string): string | null {
  const m = /InstallLocation\s+REG_SZ\s+(.+)$/im.exec(regOutput);
  if (!m) return null;
  const loc = m[1].trim().replace(/^"+|"+$/g, '');
  return loc || null;
}

function queryInstallLocation(hivePath: string): string | null {
  try {
    const out = execFileSync('reg', ['query', hivePath, '/v', 'InstallLocation'], {
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
      timeout: 3000,
      encoding: 'utf8',
    });
    return parseInstallLocation(String(out));
  } catch {
    return null;
  }
}

export function readInnoInstallLocation(
  query: (hivePath: string) => string | null = queryInstallLocation,
): string | null {
  for (const hive of uninstallHives()) {
    const loc = query(hive);
    if (loc) return loc;
  }
  return null;
}

export function pathsAreSameInstall(a: string, b: string): boolean {
  const norm = (p: string) =>
    path.resolve(p).replace(/[/\\]+$/, '').toLowerCase();
  return norm(a) === norm(b);
}

/**
 * Inno when this tree has Inno uninstall markers, or when the AppId
 * uninstall key's InstallLocation is this tree. A zip extract on a machine
 * that also has a Setup install stays portable (do not spawn Setup here).
 */
export function detectInstallLayout(
  installRoot: string,
  opts?: { registryInstallLocation?: string | null },
): InstallLayout {
  if (hasInnoMarkers(installRoot)) return 'inno';
  const loc =
    opts && 'registryInstallLocation' in opts
      ? (opts.registryInstallLocation ?? null)
      : readInnoInstallLocation();
  if (loc && pathsAreSameInstall(installRoot, loc)) return 'inno';
  return 'portable';
}
