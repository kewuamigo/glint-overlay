import fs from 'node:fs/promises';
import path from 'node:path';
import { readRevisionManifest } from '../pack.js';
import type { StorageProvider } from '../provider.js';
import { packTar, unpackTar } from '../tar.js';
import type { SelfHostStorageConfig, SyncRevisionInfo } from '../types.js';
import { assertRevisionId, resolveGameId, sanitizeGameId } from './ids.js';

export class SelfHostStorageProvider implements StorageProvider {
  constructor(private readonly config: SelfHostStorageConfig) {}

  private base(): string {
    return this.config.baseUrl.replace(/\/+$/, '');
  }

  private headers(extra?: Record<string, string>): Record<string, string> {
    return {
      Authorization: `Bearer ${this.config.bearerToken}`,
      ...extra,
    };
  }

  private revUrl(gameId: string, revisionId?: string): string {
    const g = encodeURIComponent(sanitizeGameId(gameId));
    const root = `${this.base()}/v1/games/${g}/revisions`;
    if (revisionId === undefined) return root;
    return `${root}/${encodeURIComponent(assertRevisionId(revisionId))}`;
  }

  async putRevision(gameId: string, revisionDir: string): Promise<SyncRevisionInfo> {
    const manifest = await readRevisionManifest(revisionDir);
    const id = resolveGameId(gameId, manifest.gameId);
    const tar = await packTar(revisionDir);
    const res = await fetch(this.revUrl(id, manifest.revisionId), {
      method: 'PUT',
      headers: this.headers({ 'Content-Type': 'application/x-tar' }),
      body: new Uint8Array(tar),
    });
    if (!res.ok) {
      throw new Error(
        `self-host putRevision failed: ${res.status} ${await res.text()}`,
      );
    }
    return (await res.json()) as SyncRevisionInfo;
  }

  async getRevision(
    gameId: string,
    revisionId: string,
    destDir: string,
  ): Promise<string> {
    const res = await fetch(this.revUrl(sanitizeGameId(gameId), revisionId), {
      headers: this.headers(),
    });
    if (!res.ok) {
      throw new Error(
        `self-host getRevision failed: ${res.status} ${await res.text()}`,
      );
    }
    const tar = Buffer.from(await res.arrayBuffer());
    const out = path.join(destDir, revisionId);
    await fs.rm(out, { recursive: true, force: true });
    await unpackTar(tar, out);
    return out;
  }

  async listRevisions(gameId: string): Promise<SyncRevisionInfo[]> {
    const res = await fetch(this.revUrl(sanitizeGameId(gameId)), {
      headers: this.headers(),
    });
    if (!res.ok) {
      throw new Error(
        `self-host listRevisions failed: ${res.status} ${await res.text()}`,
      );
    }
    const body = (await res.json()) as { revisions: SyncRevisionInfo[] };
    return body.revisions ?? [];
  }

  async deleteRevision(gameId: string, revisionId: string): Promise<void> {
    const res = await fetch(this.revUrl(sanitizeGameId(gameId), revisionId), {
      method: 'DELETE',
      headers: this.headers(),
    });
    if (!res.ok && res.status !== 404) {
      throw new Error(
        `self-host deleteRevision failed: ${res.status} ${await res.text()}`,
      );
    }
  }
}

/** Probe auth + reachability without uploading a revision pack. */
export async function testSelfHostConnection(
  config: SelfHostStorageConfig,
): Promise<void> {
  if (!config.baseUrl?.trim()) throw new Error('self-host baseUrl is required');
  if (!config.bearerToken?.trim()) {
    throw new Error('self-host bearerToken is required');
  }
  const base = config.baseUrl.replace(/\/+$/, '');
  const res = await fetch(`${base}/v1/games/_connection-test/revisions`, {
    headers: { Authorization: `Bearer ${config.bearerToken}` },
  });
  if (res.status === 401) {
    throw new Error('self-host auth failed: invalid bearer token');
  }
  if (!res.ok) {
    throw new Error(
      `self-host connection failed: ${res.status} ${await res.text()}`,
    );
  }
}
