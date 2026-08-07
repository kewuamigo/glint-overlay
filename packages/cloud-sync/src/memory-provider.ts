import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { readRevisionManifest } from './pack.js';
import type { StorageProvider } from './provider.js';
import type { SyncRevisionInfo } from './types.js';

/**
 * Disk-backed temp-dir provider for unit tests — no network.
 * Name kept for stable exports; storage is on disk under `root`.
 */
export class MemoryStorageProvider implements StorageProvider {
  private readonly root: string;
  private readonly games = new Map<string, Map<string, string>>();

  constructor(root?: string) {
    this.root = root ?? path.join(os.tmpdir(), `go-cloud-sync-${process.pid}`);
  }

  private gameMap(gameId: string): Map<string, string> {
    let m = this.games.get(gameId);
    if (!m) {
      m = new Map();
      this.games.set(gameId, m);
    }
    return m;
  }

  async putRevision(gameId: string, revisionDir: string): Promise<SyncRevisionInfo> {
    const manifest = await readRevisionManifest(revisionDir);
    if (manifest.gameId !== gameId) {
      throw new Error(
        `Revision gameId mismatch: expected ${gameId}, got ${manifest.gameId}`,
      );
    }
    const dest = path.join(this.root, gameId, manifest.revisionId);
    await fs.rm(dest, { recursive: true, force: true });
    await fs.mkdir(path.dirname(dest), { recursive: true });
    await fs.cp(revisionDir, dest, { recursive: true });
    this.gameMap(gameId).set(manifest.revisionId, dest);
    return {
      gameId,
      revisionId: manifest.revisionId,
      createdAt: manifest.createdAt,
      contentHash: manifest.contentHash,
    };
  }

  async getRevision(
    gameId: string,
    revisionId: string,
    destDir: string,
  ): Promise<string> {
    const src = this.gameMap(gameId).get(revisionId);
    if (!src) {
      throw new Error(`Revision not found: ${gameId}/${revisionId}`);
    }
    const out = path.join(destDir, revisionId);
    await fs.rm(out, { recursive: true, force: true });
    await fs.mkdir(destDir, { recursive: true });
    await fs.cp(src, out, { recursive: true });
    return out;
  }

  async listRevisions(gameId: string): Promise<SyncRevisionInfo[]> {
    const m = this.gameMap(gameId);
    const list: SyncRevisionInfo[] = [];
    for (const [revisionId, dir] of m) {
      const manifest = await readRevisionManifest(dir);
      list.push({
        gameId,
        revisionId,
        createdAt: manifest.createdAt,
        contentHash: manifest.contentHash,
      });
    }
    list.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    return list;
  }

  async deleteRevision(gameId: string, revisionId: string): Promise<void> {
    const m = this.gameMap(gameId);
    const dir = m.get(revisionId);
    if (!dir) return;
    await fs.rm(dir, { recursive: true, force: true });
    m.delete(revisionId);
  }
}
