import fs from 'node:fs';
import path from 'node:path';
import { APP_DATA_DIR_NAME } from './product.js';

/**
 * Survives UAC elevate — read by Rust spawn_overlay_host.
 * Logs/debug live under `%LOCALAPPDATA%/Glint` (no copy from legacy
 * `GameOverlay` — one-time path change; old logs stay put).
 */
export function overlayDebugFlagPath(): string {
  const base = process.env.LOCALAPPDATA || process.env.USERPROFILE || '.';
  return path.join(base, APP_DATA_DIR_NAME, 'overlay-debug');
}

export function syncOverlayDebugFlag(enabled: boolean): void {
  const flag = overlayDebugFlagPath();
  fs.mkdirSync(path.dirname(flag), { recursive: true });
  if (enabled) {
    fs.writeFileSync(flag, '1', 'utf8');
  } else {
    try {
      fs.unlinkSync(flag);
    } catch {
      // absent
    }
  }
}

export function overlayLogsDir(): string {
  const base = process.env.LOCALAPPDATA || process.env.USERPROFILE || '.';
  return path.join(base, APP_DATA_DIR_NAME, 'logs');
}
