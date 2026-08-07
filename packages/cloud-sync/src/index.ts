export type {
  SyncSaveRootEntry,
  SyncRevisionManifest,
  SyncRevisionInfo,
  PackRevisionInput,
  PackRevisionResult,
  UnpackRevisionResult,
  SelfHostStorageConfig,
  DriveStorageConfig,
  FtpStorageConfig,
  CloudStorageConfig,
} from './types.js';

export {
  packRevision,
  unpackRevision,
  readRevisionManifest,
  hashTree,
  type UnpackOptions,
} from './pack.js';

export type { StorageProvider } from './provider.js';
export { MemoryStorageProvider } from './memory-provider.js';

export {
  createStorageProvider,
  testStorageConnection,
  SelfHostStorageProvider,
  FtpStorageProvider,
  DriveStorageProvider,
  testSelfHostConnection,
  testFtpConnection,
  testDriveConnection,
  sanitizeGameId,
  resolveGameId,
  type TestConnectionResult,
} from './adapters/index.js';

export { packTar, unpackTar } from './tar.js';
