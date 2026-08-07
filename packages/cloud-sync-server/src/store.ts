import fs from 'node:fs/promises';
import path from 'node:path';
import {
  readRevisionManifest,
  type SyncRevisionInfo,
} from '@glint/cloud-sync';
import { packTar, unpackTar } from './tar.js';

const ID_RE = /^[a-zA-Z0-9._-]{1,128}$/;

export function assertSafeId(kind: string, id: string): string {
  if (!ID_RE.test(id)) {
    throw Object.assign(new Error(`Invalid ${kind}`), { status: 400 });
  }
  return id;
}

export class DiskRevisionStore {
  constructor(readonly root: string) {}

  private gameDir(gameId: string): string {
    return path.join(this.root, assertSafeId('gameId', gameId));
  }

  private revisionDir(gameId: string, revisionId: string): string {
    return path.join(
      this.gameDir(gameId),
      assertSafeId('revisionId', revisionId),
    );
  }

  async putTar(gameId: string, revisionId: string, tar: Buffer): Promise<SyncRevisionInfo> {
    const dest = this.revisionDir(gameId, revisionId);
    const tmp = `${dest}.tmp-${process.pid}-${Date.now()}`;
    await fs.rm(tmp, { recursive: true, force: true });
    await fs.mkdir(tmp, { recursive: true });
    try {
      await unpackTar(tar, tmp);
      const manifest = await readRevisionManifest(tmp);
      if (manifest.gameId !== gameId) {
        throw Object.assign(
          new Error(
            `Revision gameId mismatch: expected ${gameId}, got ${manifest.gameId}`,
          ),
          { status: 400 },
        );
      }
      if (manifest.revisionId !== revisionId) {
        throw Object.assign(
          new Error(
            `Revision id mismatch: expected ${revisionId}, got ${manifest.revisionId}`,
          ),
          { status: 400 },
        );
      }
      await fs.mkdir(this.gameDir(gameId), { recursive: true });
      await fs.rm(dest, { recursive: true, force: true });
      await fs.rename(tmp, dest);
      return {
        gameId: manifest.gameId,
        revisionId: manifest.revisionId,
        createdAt: manifest.createdAt,
        contentHash: manifest.contentHash,
      };
    } catch (err) {
      await fs.rm(tmp, { recursive: true, force: true });
      throw err;
    }
  }

  async getTar(gameId: string, revisionId: string): Promise<Buffer> {
    const dest = this.revisionDir(gameId, revisionId);
    try {
      await fs.access(path.join(dest, 'manifest.json'));
    } catch {
      throw Object.assign(new Error('Revision not found'), { status: 404 });
    }
    return packTar(dest);
  }

  async list(gameId: string): Promise<SyncRevisionInfo[]> {
    const dir = this.gameDir(gameId);
    let names: string[];
    try {
      names = await fs.readdir(dir);
    } catch {
      return [];
    }
    const list: SyncRevisionInfo[] = [];
    for (const revisionId of names) {
      if (!ID_RE.test(revisionId)) continue;
      try {
        const manifest = await readRevisionManifest(path.join(dir, revisionId));
        list.push({
          gameId: manifest.gameId,
          revisionId: manifest.revisionId,
          createdAt: manifest.createdAt,
          contentHash: manifest.contentHash,
        });
      } catch {
        // skip corrupt / incomplete dirs
      }
    }
    list.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    return list;
  }

  async delete(gameId: string, revisionId: string): Promise<void> {
    const dest = this.revisionDir(gameId, revisionId);
    try {
      await fs.access(path.join(dest, 'manifest.json'));
    } catch {
      throw Object.assign(new Error('Revision not found'), { status: 404 });
    }
    await fs.rm(dest, { recursive: true, force: true });
  }
}
