import fs from 'node:fs';
import path from 'node:path';
import {
  APP_DATA_DIR_NAME,
  LEGACY_APP_DATA_DIR_NAME,
  MIGRATION_MARKER,
} from './product.js';

export function glintAppDataRoot(appData = process.env.APPDATA ?? ''): string {
  return path.join(appData, APP_DATA_DIR_NAME);
}

export function legacyAppDataRoot(appData = process.env.APPDATA ?? ''): string {
  return path.join(appData, LEGACY_APP_DATA_DIR_NAME);
}

/**
 * Copy-once: if Glint root has no migration marker and legacy GameOverlay exists,
 * copy the tree into Glint and write the marker. Never overwrite an already-migrated Glint root.
 */
export function migrateAppDataFromGameOverlay(
  appData = process.env.APPDATA ?? '',
): { migrated: boolean; reason: string } {
  const dest = glintAppDataRoot(appData);
  const src = legacyAppDataRoot(appData);
  const marker = path.join(dest, MIGRATION_MARKER);

  if (fs.existsSync(marker)) {
    return { migrated: false, reason: 'already-migrated' };
  }
  if (!fs.existsSync(src)) {
    fs.mkdirSync(dest, { recursive: true });
    fs.writeFileSync(marker, `created=${new Date().toISOString()}\n`, 'utf8');
    return { migrated: false, reason: 'no-legacy' };
  }

  fs.mkdirSync(dest, { recursive: true });
  fs.cpSync(src, dest, {
    recursive: true,
    force: false,
    errorOnExist: false,
  });
  fs.writeFileSync(
    marker,
    `from=${src}\nat=${new Date().toISOString()}\n`,
    'utf8',
  );
  return { migrated: true, reason: 'copied' };
}
