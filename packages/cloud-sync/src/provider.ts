import type { SyncRevisionInfo } from './types.js';

/**
 * Interchangeable cloud backend for sync revisions (design D2).
 * Packing is independent of this interface.
 */
export interface StorageProvider {
  putRevision(gameId: string, revisionDir: string): Promise<SyncRevisionInfo>;
  getRevision(
    gameId: string,
    revisionId: string,
    destDir: string,
  ): Promise<string>;
  listRevisions(gameId: string): Promise<SyncRevisionInfo[]>;
  deleteRevision(gameId: string, revisionId: string): Promise<void>;
}
