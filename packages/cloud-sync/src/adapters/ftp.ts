import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { Client } from 'basic-ftp';
import { readRevisionManifest } from '../pack.js';
import type { StorageProvider } from '../provider.js';
import { packTar, unpackTar } from '../tar.js';
import type { FtpStorageConfig, SyncRevisionInfo } from '../types.js';
import { assertRevisionId, resolveGameId, sanitizeGameId } from './ids.js';

function warnIfPlainFtp(config: FtpStorageConfig): void {
  if (!config.secure) {
    console.warn(
      '[cloud-sync] FTP secure=false: credentials and saves travel in cleartext. Prefer FTPS (secure: true).',
    );
  }
}

function joinRemote(...parts: string[]): string {
  return parts
    .map((p) => p.replace(/^\/+|\/+$/g, ''))
    .filter(Boolean)
    .join('/');
}

async function withClient<T>(
  config: FtpStorageConfig,
  fn: (client: Client) => Promise<T>,
): Promise<T> {
  warnIfPlainFtp(config);
  const client = new Client(30_000);
  try {
    await client.access({
      host: config.host,
      user: config.user,
      password: config.password,
      secure: config.secure,
      port: config.port,
    });
    return await fn(client);
  } finally {
    client.close();
  }
}

/**
 * Layout under basePath:
 *   {gameId}/{revisionId}.tar
 *   {gameId}/{revisionId}.json   // SyncRevisionInfo sidecar for list
 */
export class FtpStorageProvider implements StorageProvider {
  constructor(private readonly config: FtpStorageConfig) {}

  private gameDir(gameId: string): string {
    return joinRemote(this.config.basePath, sanitizeGameId(gameId));
  }

  async putRevision(gameId: string, revisionDir: string): Promise<SyncRevisionInfo> {
    const manifest = await readRevisionManifest(revisionDir);
    const id = resolveGameId(gameId, manifest.gameId);
    assertRevisionId(manifest.revisionId);
    const info: SyncRevisionInfo = {
      gameId: id,
      revisionId: manifest.revisionId,
      createdAt: manifest.createdAt,
      contentHash: manifest.contentHash,
    };
    const tar = await packTar(revisionDir);
    const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-ftp-put-'));
    try {
      const tarPath = path.join(tmp, 'rev.tar');
      const metaPath = path.join(tmp, 'rev.json');
      await fs.writeFile(tarPath, tar);
      await fs.writeFile(metaPath, JSON.stringify(info));
      const dir = this.gameDir(id);
      await withClient(this.config, async (client) => {
        await client.ensureDir(dir);
        await client.uploadFrom(
          tarPath,
          joinRemote(dir, `${manifest.revisionId}.tar`),
        );
        await client.uploadFrom(
          metaPath,
          joinRemote(dir, `${manifest.revisionId}.json`),
        );
      });
    } finally {
      await fs.rm(tmp, { recursive: true, force: true });
    }
    return info;
  }

  async getRevision(
    gameId: string,
    revisionId: string,
    destDir: string,
  ): Promise<string> {
    assertRevisionId(revisionId);
    const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-ftp-get-'));
    try {
      const tarPath = path.join(tmp, 'rev.tar');
      const remote = joinRemote(this.gameDir(gameId), `${revisionId}.tar`);
      await withClient(this.config, async (client) => {
        await client.downloadTo(tarPath, remote);
      });
      const tar = await fs.readFile(tarPath);
      const out = path.join(destDir, revisionId);
      await fs.rm(out, { recursive: true, force: true });
      await unpackTar(tar, out);
      return out;
    } finally {
      await fs.rm(tmp, { recursive: true, force: true });
    }
  }

  async listRevisions(gameId: string): Promise<SyncRevisionInfo[]> {
    const dir = this.gameDir(gameId);
    return withClient(this.config, async (client) => {
      let listing;
      try {
        listing = await client.list(dir);
      } catch {
        return [];
      }
      const list: SyncRevisionInfo[] = [];
      const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-ftp-list-'));
      try {
        for (const entry of listing) {
          if (!entry.name.endsWith('.json')) continue;
          const local = path.join(tmp, entry.name);
          try {
            await client.downloadTo(local, joinRemote(dir, entry.name));
            const info = JSON.parse(
              await fs.readFile(local, 'utf8'),
            ) as SyncRevisionInfo;
            list.push(info);
          } catch {
            // skip corrupt sidecars
          }
        }
      } finally {
        await fs.rm(tmp, { recursive: true, force: true });
      }
      list.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
      return list;
    });
  }

  async deleteRevision(gameId: string, revisionId: string): Promise<void> {
    assertRevisionId(revisionId);
    const dir = this.gameDir(gameId);
    await withClient(this.config, async (client) => {
      for (const ext of ['.tar', '.json'] as const) {
        try {
          await client.remove(joinRemote(dir, `${revisionId}${ext}`));
        } catch {
          // ignore missing
        }
      }
    });
  }
}

/** Login + optional cwd into basePath — no revision upload. */
export async function testFtpConnection(config: FtpStorageConfig): Promise<void> {
  if (!config.host?.trim()) throw new Error('FTP host is required');
  if (!config.user?.trim()) throw new Error('FTP user is required');
  try {
    await withClient(config, async (client) => {
      const base = config.basePath?.replace(/^\/+|\/+$/g, '') ?? '';
      if (base) {
        await client.ensureDir(base);
      } else {
        await client.pwd();
      }
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    throw new Error(`FTP connection failed: ${msg}`);
  }
}
