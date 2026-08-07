import fs from 'node:fs';
import path from 'node:path';
import type { CloudStorageConfig } from '@glint/cloud-sync';
import {
  createStorageProvider,
  testStorageConnection,
  type TestConnectionResult,
} from '@glint/cloud-sync';
import { APP_DATA_DIR_NAME } from './product.js';

export type CloudStorageState = {
  /** Cloud sync master switch (default off until user enables). */
  enabled: boolean;
  /** Active provider config, or null when unset */
  config: CloudStorageConfig | null;
};

const DEFAULT_STATE: CloudStorageState = { enabled: false, config: null };

function configPath(): string {
  return path.join(
    process.env.APPDATA ?? '',
    APP_DATA_DIR_NAME,
    'cloud-storage.json',
  );
}

export function loadCloudStorageConfig(): CloudStorageState {
  const file = configPath();
  try {
    if (!fs.existsSync(file)) return { ...DEFAULT_STATE };
    const raw = JSON.parse(fs.readFileSync(file, 'utf8')) as Partial<CloudStorageState>;
    if (!raw || typeof raw !== 'object') return { ...DEFAULT_STATE };
    return {
      enabled: raw.enabled === true,
      config: raw.config ?? null,
    };
  } catch {
    return { ...DEFAULT_STATE };
  }
}

export function saveCloudStorageConfig(
  config: CloudStorageConfig | null,
  enabled?: boolean,
): CloudStorageState {
  const prev = loadCloudStorageConfig();
  const state: CloudStorageState = {
    enabled: enabled === undefined ? prev.enabled : enabled,
    config,
  };
  const file = configPath();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(state, null, 2), 'utf8');
  return state;
}

export function setCloudSyncEnabled(enabled: boolean): CloudStorageState {
  const prev = loadCloudStorageConfig();
  return saveCloudStorageConfig(prev.config, enabled);
}

export function isCloudSyncReady(): boolean {
  const { enabled, config } = loadCloudStorageConfig();
  return enabled === true && config != null;
}

export function getActiveStorageProvider() {
  const { config } = loadCloudStorageConfig();
  if (!config) return null;
  return createStorageProvider(config);
}

export async function testCloudStorageConnection(
  override?: CloudStorageConfig | null,
): Promise<TestConnectionResult> {
  const config = override ?? loadCloudStorageConfig().config;
  if (!config) {
    throw new Error('No cloud storage provider configured');
  }
  const result = await testStorageConnection(config);
  // Persist Drive folder id when resolved so later syncs skip lookup.
  if (result.folderId && config.provider === 'drive' && !config.folderId) {
    saveCloudStorageConfig({
      ...config,
      folderId: result.folderId,
    });
  }
  return result;
}
