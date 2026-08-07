import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { Client } from 'basic-ftp';
import {
  DriveStorageProvider,
  FtpStorageProvider,
  SelfHostStorageProvider,
  packRevision,
  resolveGameId,
  sanitizeGameId,
  createStorageProvider,
  testStorageConnection,
  unpackTar,
  readRevisionManifest,
} from '../dist/index.js';

test('sanitizeGameId keeps safe ids and rewrites unsafe', () => {
  assert.equal(sanitizeGameId('demo-game'), 'demo-game');
  assert.equal(sanitizeGameId('custom:C:\\Games\\x'), 'custom_C__Games_x');
  assert.match(sanitizeGameId('custom:C:\\Games\\x'), /^[a-zA-Z0-9._-]{1,128}$/);
});

test('resolveGameId requires pack to use sanitized id', () => {
  const raw = 'custom:C:\\Games\\x';
  const id = sanitizeGameId(raw);
  assert.equal(resolveGameId(raw, id), id);
  assert.throws(() => resolveGameId(raw, raw), /Pack with sanitizeGameId/);
});

test('createStorageProvider builds each adapter', () => {
  assert.equal(
    createStorageProvider({
      provider: 'selfhost',
      baseUrl: 'http://127.0.0.1',
      bearerToken: 't',
    }).constructor.name,
    'SelfHostStorageProvider',
  );
  assert.equal(
    createStorageProvider({
      provider: 'ftp',
      host: '127.0.0.1',
      user: 'u',
      password: 'p',
      basePath: '/sync',
      secure: true,
    }).constructor.name,
    'FtpStorageProvider',
  );
  assert.equal(
    createStorageProvider({
      provider: 'drive',
      accessToken: 'tok',
    }).constructor.name,
    'DriveStorageProvider',
  );
});

test('Drive testConnection fails clearly without token', async () => {
  await assert.rejects(
    () =>
      testStorageConnection({
        provider: 'drive',
        accessToken: '',
      }),
    /auth error|accessToken is missing/,
  );
});

/** Minimal self-host API mock for put only. */
function startPutMock(token) {
  /** @type {Map<string, Buffer>} */
  const blobs = new Map();
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    if (req.headers.authorization !== `Bearer ${token}`) {
      res.writeHead(401);
      res.end();
      return;
    }
    const revMatch = url.pathname.match(
      /^\/v1\/games\/([^/]+)\/revisions\/([^/]+)\/?$/,
    );
    if (req.method === 'PUT' && revMatch) {
      const gameId = decodeURIComponent(revMatch[1]);
      const revisionId = decodeURIComponent(revMatch[2]);
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const body = Buffer.concat(chunks);
      blobs.set(`${gameId}/${revisionId}`, body);
      const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-gid-'));
      try {
        await unpackTar(body, tmp);
        const m = await readRevisionManifest(tmp);
        res.writeHead(201, { 'Content-Type': 'application/json' });
        res.end(
          JSON.stringify({
            gameId: m.gameId,
            revisionId: m.revisionId,
            createdAt: m.createdAt,
            contentHash: m.contentHash,
          }),
        );
      } finally {
        await fs.rm(tmp, { recursive: true, force: true });
      }
      return;
    }
    res.writeHead(404);
    res.end();
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      resolve({
        server,
        baseUrl: `http://127.0.0.1:${addr.port}`,
        blobs,
      });
    });
  });
}

test('same gameId string succeeds on selfhost, ftp (stub), and drive (mock)', async () => {
  const raw = 'custom:C:\\Games\\x';
  const id = sanitizeGameId(raw);
  assert.equal(id, 'custom_C__Games_x');

  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-gid-pack-'));
  const saveRoot = path.join(tmp, 'saves');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'a.txt'), 'payload');
  const { revisionDir } = await packRevision(path.join(tmp, 'pack'), {
    gameId: id,
    saveLocations: [saveRoot],
    revisionId: '2026-02-01T00-00-00-000Z',
    createdAt: new Date('2026-02-01T00:00:00.000Z'),
  });

  // --- selfhost ---
  const { server, baseUrl, blobs } = await startPutMock('tok');
  try {
    const selfhost = new SelfHostStorageProvider({
      provider: 'selfhost',
      baseUrl,
      bearerToken: 'tok',
    });
    const info = await selfhost.putRevision(raw, revisionDir);
    assert.equal(info.gameId, id);
    assert.ok(blobs.has(`${id}/2026-02-01T00-00-00-000Z`));
  } finally {
    server.close();
  }

  // --- drive (fetch mock) ---
  const origFetch = globalThis.fetch;
  /** @type {string[]} */
  const driveNames = [];
  globalThis.fetch = async (input, init) => {
    const url = String(input);
    if (url.includes('/about')) {
      return new Response(JSON.stringify({ user: { displayName: 't' } }), {
        status: 200,
      });
    }
    if (url.includes('/files?q=')) {
      // findFileId / list — no existing revision
      return new Response(JSON.stringify({ files: [] }), { status: 200 });
    }
    if (url.includes('uploadType=multipart')) {
      const body = init?.body;
      const text =
        body instanceof Uint8Array
          ? Buffer.from(body).toString('utf8')
          : String(body ?? '');
      const nameMatch = text.match(/"name"\s*:\s*"([^"]+)"/);
      if (nameMatch) driveNames.push(nameMatch[1]);
      return new Response(JSON.stringify({ id: 'file1' }), { status: 200 });
    }
    if (url.includes('/files/') && init?.method === 'DELETE') {
      return new Response(null, { status: 204 });
    }
    return new Response('unexpected', { status: 500 });
  };
  try {
    const drive = new DriveStorageProvider({
      provider: 'drive',
      accessToken: 'paste-token',
      folderId: 'folder1',
    });
    const info = await drive.putRevision(raw, revisionDir);
    assert.equal(info.gameId, id);
    assert.equal(driveNames[0], `${id}__2026-02-01T00-00-00-000Z.tar`);
  } finally {
    globalThis.fetch = origFetch;
  }

  // --- ftp (stub Client I/O) ---
  const uploads = [];
  const access = Client.prototype.access;
  const ensureDir = Client.prototype.ensureDir;
  const uploadFrom = Client.prototype.uploadFrom;
  const close = Client.prototype.close;
  Client.prototype.access = async function () {};
  Client.prototype.ensureDir = async function () {};
  Client.prototype.uploadFrom = async function (_local, remote) {
    uploads.push(remote);
  };
  Client.prototype.close = function () {};
  try {
    const ftp = new FtpStorageProvider({
      provider: 'ftp',
      host: '127.0.0.1',
      user: 'u',
      password: 'p',
      basePath: 'sync',
      secure: true,
    });
    const info = await ftp.putRevision(raw, revisionDir);
    assert.equal(info.gameId, id);
    assert.ok(uploads.some((r) => String(r).includes(id)));
  } finally {
    Client.prototype.access = access;
    Client.prototype.ensureDir = ensureDir;
    Client.prototype.uploadFrom = uploadFrom;
    Client.prototype.close = close;
  }

  await fs.rm(tmp, { recursive: true, force: true });
});
