import {
  createServer,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from 'node:http';
import { DiskRevisionStore } from './store.js';

export interface CloudSyncServerOptions {
  root: string;
  token: string;
  host?: string;
  port?: number;
}

function json(res: ServerResponse, status: number, body: unknown): void {
  const data = JSON.stringify(body);
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(data),
  });
  res.end(data);
}

function text(res: ServerResponse, status: number, body: string): void {
  res.writeHead(status, {
    'Content-Type': 'text/plain; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  });
  res.end(body);
}

function readBody(req: IncomingMessage, limit = 256 * 1024 * 1024): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > limit) {
        reject(Object.assign(new Error('Body too large'), { status: 413 }));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}

function authorized(req: IncomingMessage, token: string): boolean {
  const header = req.headers.authorization;
  if (!header || !header.startsWith('Bearer ')) return false;
  const got = header.slice('Bearer '.length);
  return got.length > 0 && got === token;
}

/**
 * Routes (all except GET /health require Authorization: Bearer <token>):
 *   GET    /health
 *   GET    /v1/games/:gameId/revisions
 *   PUT    /v1/games/:gameId/revisions/:revisionId   body = tar of revision dir
 *   GET    /v1/games/:gameId/revisions/:revisionId   body = tar of revision dir
 *   DELETE /v1/games/:gameId/revisions/:revisionId
 */
export function createCloudSyncServer(opts: CloudSyncServerOptions): Server {
  if (!opts.token) {
    throw new Error('CLOUD_SYNC_TOKEN is required');
  }
  const store = new DiskRevisionStore(opts.root);

  return createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`);
      const method = req.method ?? 'GET';

      if (method === 'GET' && url.pathname === '/health') {
        text(res, 200, 'ok');
        return;
      }

      if (!authorized(req, opts.token)) {
        // Reject before reading body / touching storage (spec: no store/return).
        json(res, 401, { error: 'unauthorized' });
        req.resume();
        return;
      }

      const listMatch = url.pathname.match(
        /^\/v1\/games\/([^/]+)\/revisions\/?$/,
      );
      const revMatch = url.pathname.match(
        /^\/v1\/games\/([^/]+)\/revisions\/([^/]+)\/?$/,
      );

      if (method === 'GET' && listMatch) {
        const gameId = decodeURIComponent(listMatch[1]!);
        const revisions = await store.list(gameId);
        json(res, 200, { revisions });
        return;
      }

      if (method === 'PUT' && revMatch) {
        const gameId = decodeURIComponent(revMatch[1]!);
        const revisionId = decodeURIComponent(revMatch[2]!);
        const body = await readBody(req);
        if (body.length === 0) {
          json(res, 400, { error: 'empty body' });
          return;
        }
        const info = await store.putTar(gameId, revisionId, body);
        json(res, 201, info);
        return;
      }

      if (method === 'GET' && revMatch) {
        const gameId = decodeURIComponent(revMatch[1]!);
        const revisionId = decodeURIComponent(revMatch[2]!);
        const tar = await store.getTar(gameId, revisionId);
        res.writeHead(200, {
          'Content-Type': 'application/x-tar',
          'Content-Length': tar.length,
        });
        res.end(tar);
        return;
      }

      if (method === 'DELETE' && revMatch) {
        const gameId = decodeURIComponent(revMatch[1]!);
        const revisionId = decodeURIComponent(revMatch[2]!);
        await store.delete(gameId, revisionId);
        res.writeHead(204);
        res.end();
        return;
      }

      json(res, 404, { error: 'not found' });
    } catch (err) {
      const status =
        err && typeof err === 'object' && 'status' in err
          ? Number((err as { status: number }).status)
          : 500;
      const message = err instanceof Error ? err.message : 'error';
      if (status >= 500) {
        console.error('[cloud-sync-server]', err);
      }
      json(res, status || 500, { error: message });
    }
  });
}

export async function startCloudSyncServer(
  opts: CloudSyncServerOptions,
): Promise<Server> {
  const server = createCloudSyncServer(opts);
  const host = opts.host ?? '127.0.0.1';
  const port = opts.port ?? 8787;
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, host, () => resolve());
  });
  return server;
}
