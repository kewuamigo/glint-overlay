import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { projectRoot } from './paths.js';

const DEV_FALLBACK = '0.0.0-dev';

function readVersionFile(filePath: string): string | null {
  try {
    const raw = fs.readFileSync(filePath, 'utf8');
    const data = JSON.parse(raw) as { version?: unknown };
    if (typeof data.version === 'string' && data.version.trim()) {
      return data.version.trim().replace(/^v/i, '');
    }
  } catch {
    // missing or invalid
  }
  return null;
}

function launcherPackageVersion(): string | null {
  try {
    const here = path.dirname(fileURLToPath(import.meta.url));
    const pkgPath = path.resolve(here, '../package.json');
    const raw = fs.readFileSync(pkgPath, 'utf8');
    const data = JSON.parse(raw) as { version?: unknown };
    if (typeof data.version === 'string' && data.version.trim()) {
      return data.version.trim();
    }
  } catch {
    // ignore
  }
  return null;
}

/**
 * Packaged builds bake `version.json` at payload root and next to the launcher
 * host. Dev (no bake) falls back to `host/launcher/package.json`, then `0.0.0-dev`.
 */
export function readProductVersion(root = projectRoot()): string {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const candidates = [
    path.join(root, 'version.json'),
    path.join(root, 'host', 'launcher', 'version.json'),
    path.resolve(here, '../version.json'),
  ];
  for (const file of candidates) {
    const ver = readVersionFile(file);
    if (ver) return ver;
  }
  return launcherPackageVersion() ?? DEV_FALLBACK;
}

export const DEV_VERSION_FALLBACK = DEV_FALLBACK;
