/**
 * Google Drive adapter — paste-accessToken v1.
 *
 * Obtain a short-lived access token outside the launcher (OAuth Playground,
 * gcloud, or a one-shot script with Drive scope
 * `https://www.googleapis.com/auth/drive.file`), then paste it into
 * DriveStorageConfig.accessToken.
 *
 * Optional: set `folderId` to a dedicated Drive folder; if omitted, the
 * adapter creates/finds a folder named "Glint Sync".
 *
 * Browser OAuth and token refresh are deferred. `refreshToken` is ignored in
 * v1 — when the access token expires, paste a fresh one.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import { readRevisionManifest } from '../pack.js';
import type { StorageProvider } from '../provider.js';
import { packTar, unpackTar } from '../tar.js';
import type { DriveStorageConfig, SyncRevisionInfo } from '../types.js';
import { assertRevisionId, resolveGameId, sanitizeGameId } from './ids.js';

const DRIVE_API = 'https://www.googleapis.com/drive/v3';
const DRIVE_UPLOAD = 'https://www.googleapis.com/upload/drive/v3';
const FOLDER_NAME = 'Glint Sync';

function authError(detail: string): Error {
  return new Error(`Google Drive auth error: ${detail}`);
}

function requireAccessToken(config: DriveStorageConfig): string {
  const token = config.accessToken?.trim();
  if (!token) {
    throw authError(
      'accessToken is missing. Paste a current Drive access token in cloud storage settings.',
    );
  }
  return token;
}

async function driveFetch(
  config: DriveStorageConfig,
  url: string,
  init: RequestInit = {},
): Promise<Response> {
  const token = requireAccessToken(config);
  const headers = new Headers(init.headers);
  headers.set('Authorization', `Bearer ${token}`);
  const res = await fetch(url, { ...init, headers });
  if (res.status === 401 || res.status === 403) {
    throw authError(
      `token rejected (${res.status}). Paste a fresh accessToken (v1 does not refresh).`,
    );
  }
  return res;
}

async function ensureFolderId(config: DriveStorageConfig): Promise<string> {
  if (config.folderId?.trim()) return config.folderId.trim();

  const q = encodeURIComponent(
    `name='${FOLDER_NAME}' and mimeType='application/vnd.google-apps.folder' and trashed=false`,
  );
  const listRes = await driveFetch(
    config,
    `${DRIVE_API}/files?q=${q}&fields=files(id,name)&pageSize=1`,
  );
  if (!listRes.ok) {
    throw new Error(`Drive folder lookup failed: ${listRes.status} ${await listRes.text()}`);
  }
  const listed = (await listRes.json()) as { files?: Array<{ id: string }> };
  if (listed.files?.[0]?.id) return listed.files[0].id;

  const createRes = await driveFetch(config, `${DRIVE_API}/files`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: FOLDER_NAME,
      mimeType: 'application/vnd.google-apps.folder',
    }),
  });
  if (!createRes.ok) {
    throw new Error(`Drive folder create failed: ${createRes.status} ${await createRes.text()}`);
  }
  const created = (await createRes.json()) as { id: string };
  return created.id;
}

function fileName(gameId: string, revisionId: string): string {
  return `${sanitizeGameId(gameId)}__${revisionId}.tar`;
}

export class DriveStorageProvider implements StorageProvider {
  constructor(private readonly config: DriveStorageConfig) {}

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
    const folderId = await ensureFolderId(this.config);
    const tar = await packTar(revisionDir);
    const name = fileName(id, manifest.revisionId);

    // Replace existing file with same name in folder
    const existing = await this.findFileId(folderId, name);
    if (existing) {
      await this.deleteFile(existing);
    }

    const meta = JSON.stringify({
      name,
      parents: [folderId],
      description: JSON.stringify(info),
      mimeType: 'application/x-tar',
    });
    const boundary = 'go_drive_boundary';
    const body = Buffer.concat([
      Buffer.from(
        `--${boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n${meta}\r\n` +
          `--${boundary}\r\nContent-Type: application/x-tar\r\n\r\n`,
      ),
      tar,
      Buffer.from(`\r\n--${boundary}--`),
    ]);

    const res = await driveFetch(
      this.config,
      `${DRIVE_UPLOAD}/files?uploadType=multipart&fields=id`,
      {
        method: 'POST',
        headers: {
          'Content-Type': `multipart/related; boundary=${boundary}`,
        },
        body: new Uint8Array(body),
      },
    );
    if (!res.ok) {
      throw new Error(`Drive putRevision failed: ${res.status} ${await res.text()}`);
    }
    return info;
  }

  async getRevision(
    gameId: string,
    revisionId: string,
    destDir: string,
  ): Promise<string> {
    assertRevisionId(revisionId);
    const id = sanitizeGameId(gameId);
    const folderId = await ensureFolderId(this.config);
    const fileId = await this.findFileId(folderId, fileName(id, revisionId));
    if (!fileId) throw new Error(`Revision not found: ${id}/${revisionId}`);
    const res = await driveFetch(
      this.config,
      `${DRIVE_API}/files/${fileId}?alt=media`,
    );
    if (!res.ok) {
      throw new Error(`Drive getRevision failed: ${res.status} ${await res.text()}`);
    }
    const tar = Buffer.from(await res.arrayBuffer());
    const out = path.join(destDir, revisionId);
    await fs.rm(out, { recursive: true, force: true });
    await unpackTar(tar, out);
    return out;
  }

  async listRevisions(gameId: string): Promise<SyncRevisionInfo[]> {
    const id = sanitizeGameId(gameId);
    const folderId = await ensureFolderId(this.config);
    const prefix = `${id}__`;
    const q = encodeURIComponent(
      `'${folderId}' in parents and trashed=false and name contains '${prefix}'`,
    );
    const res = await driveFetch(
      this.config,
      `${DRIVE_API}/files?q=${q}&fields=files(id,name,description)&pageSize=1000`,
    );
    if (!res.ok) {
      throw new Error(`Drive listRevisions failed: ${res.status} ${await res.text()}`);
    }
    const body = (await res.json()) as {
      files?: Array<{ name: string; description?: string }>;
    };
    const list: SyncRevisionInfo[] = [];
    for (const f of body.files ?? []) {
      if (!f.name.startsWith(prefix) || !f.name.endsWith('.tar')) continue;
      if (f.description) {
        try {
          list.push(JSON.parse(f.description) as SyncRevisionInfo);
          continue;
        } catch {
          // fall through
        }
      }
      const revisionId = f.name.slice(prefix.length, -'.tar'.length);
      list.push({
        gameId: id,
        revisionId,
        createdAt: '',
        contentHash: '',
      });
    }
    list.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    return list;
  }

  async deleteRevision(gameId: string, revisionId: string): Promise<void> {
    assertRevisionId(revisionId);
    const folderId = await ensureFolderId(this.config);
    const fileId = await this.findFileId(
      folderId,
      fileName(sanitizeGameId(gameId), revisionId),
    );
    if (fileId) await this.deleteFile(fileId);
  }

  private async findFileId(folderId: string, name: string): Promise<string | null> {
    const q = encodeURIComponent(
      `'${folderId}' in parents and name='${name}' and trashed=false`,
    );
    const res = await driveFetch(
      this.config,
      `${DRIVE_API}/files?q=${q}&fields=files(id)&pageSize=1`,
    );
    if (!res.ok) {
      throw new Error(`Drive find failed: ${res.status} ${await res.text()}`);
    }
    const body = (await res.json()) as { files?: Array<{ id: string }> };
    return body.files?.[0]?.id ?? null;
  }

  private async deleteFile(fileId: string): Promise<void> {
    const res = await driveFetch(this.config, `${DRIVE_API}/files/${fileId}`, {
      method: 'DELETE',
    });
    if (!res.ok && res.status !== 404) {
      throw new Error(`Drive delete failed: ${res.status} ${await res.text()}`);
    }
  }
}

/** Validate token via Drive about.get; ensure sync folder exists. */
export async function testDriveConnection(
  config: DriveStorageConfig,
): Promise<{ folderId: string }> {
  requireAccessToken(config);
  const about = await driveFetch(
    config,
    `${DRIVE_API}/about?fields=user(displayName,emailAddress)`,
  );
  if (!about.ok) {
    throw new Error(`Drive connection failed: ${about.status} ${await about.text()}`);
  }
  const folderId = await ensureFolderId(config);
  return { folderId };
}
