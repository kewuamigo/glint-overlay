import type { StorageProvider } from '../provider.js';
import type { CloudStorageConfig } from '../types.js';
import { DriveStorageProvider, testDriveConnection } from './drive.js';
import { FtpStorageProvider, testFtpConnection } from './ftp.js';
import {
  SelfHostStorageProvider,
  testSelfHostConnection,
} from './selfhost.js';

export { SelfHostStorageProvider, testSelfHostConnection } from './selfhost.js';
export { FtpStorageProvider, testFtpConnection } from './ftp.js';
export { DriveStorageProvider, testDriveConnection } from './drive.js';
export { sanitizeGameId, resolveGameId } from './ids.js';

export function createStorageProvider(
  config: CloudStorageConfig,
): StorageProvider {
  switch (config.provider) {
    case 'selfhost':
      return new SelfHostStorageProvider(config);
    case 'ftp':
      return new FtpStorageProvider(config);
    case 'drive':
      return new DriveStorageProvider(config);
    default: {
      const _exhaustive: never = config;
      throw new Error(`Unknown storage provider: ${(_exhaustive as CloudStorageConfig).provider}`);
    }
  }
}

export type TestConnectionResult = {
  ok: true;
  /** Present when Drive created/resolved the sync folder */
  folderId?: string;
};

/** Probe the configured backend without uploading a game pack. */
export async function testStorageConnection(
  config: CloudStorageConfig,
): Promise<TestConnectionResult> {
  switch (config.provider) {
    case 'selfhost':
      await testSelfHostConnection(config);
      return { ok: true };
    case 'ftp':
      await testFtpConnection(config);
      return { ok: true };
    case 'drive': {
      const { folderId } = await testDriveConnection(config);
      return { ok: true, folderId };
    }
    default: {
      const _exhaustive: never = config;
      throw new Error(`Unknown storage provider: ${(_exhaustive as CloudStorageConfig).provider}`);
    }
  }
}
