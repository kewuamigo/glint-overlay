import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {
  SelfHostStorageProvider,
  packRevision,
  packTar,
  unpackTar,
  readRevisionManifest,
  testSelfHostConnection,
} from '../dist/index.js';

async function makeTemp() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'go-selfhost-'));
}

/** Minimal self-host API mock (tar round-trip, bearer auth). */
function startMockServer(token) {
  /** @type {Map<string, Buffer>} */
  const blobs = new Map();
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    const auth = req.headers.authorization;
    if (url.pathname === '/health') {
      res.writeHead(200);
      res.end('ok');
      return;
    }
    if (auth !== `Bearer ${token}`) {
      res.writeHead(401, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'unauthorized' }));
      return;
    }
    const listMatch = url.pathname.match(/^\/v1\/games\/([^/]+)\/revisions\/?$/);
    const revMatch = url.pathname.match(
      /^\/v1\/games\/([^/]+)\/revisions\/([^/]+)\/?$/,
    );
    const readBody = () =>
      new Promise((resolve, reject) => {
        const chunks = [];
        req.on('data', (c) => chunks.push(c));
        req.on('end', () => resolve(Buffer.concat(chunks)));
        req.on('error', reject);
      });

    if (req.method === 'GET' && listMatch) {
      const gameId = decodeURIComponent(listMatch[1]);
      const revisions = [];
      for (const [key, buf] of blobs) {
        if (!key.startsWith(`${gameId}/`)) continue;
        const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-mock-'));
        try {
          await unpackTar(buf, tmp);
          const m = await readRevisionManifest(tmp);
          revisions.push({
            gameId: m.gameId,
            revisionId: m.revisionId,
            createdAt: m.createdAt,
            contentHash: m.contentHash,
          });
        } finally {
          await fs.rm(tmp, { recursive: true, force: true });
        }
      }
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ revisions }));
      return;
    }

    if (req.method === 'PUT' && revMatch) {
      const gameId = decodeURIComponent(revMatch[1]);
      const revisionId = decodeURIComponent(revMatch[2]);
      const body = await readBody();
      blobs.set(`${gameId}/${revisionId}`, body);
      const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'go-mock-'));
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

    if (req.method === 'GET' && revMatch) {
      const key = `${decodeURIComponent(revMatch[1])}/${decodeURIComponent(revMatch[2])}`;
      const buf = blobs.get(key);
      if (!buf) {
        res.writeHead(404, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: 'not found' }));
        return;
      }
      res.writeHead(200, {
        'Content-Type': 'application/x-tar',
        'Content-Length': buf.length,
      });
      res.end(buf);
      return;
    }

    if (req.method === 'DELETE' && revMatch) {
      const key = `${decodeURIComponent(revMatch[1])}/${decodeURIComponent(revMatch[2])}`;
      blobs.delete(key);
      res.writeHead(204);
      res.end();
      return;
    }

    res.writeHead(404, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'not found' }));
  });

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      resolve({
        server,
        baseUrl: `http://127.0.0.1:${addr.port}`,
      });
    });
  });
}

test('SelfHostStorageProvider put/list/get/delete against mock server', async () => {
  const tmp = await makeTemp();
  const { server, baseUrl } = await startMockServer('test-token');
  try {
    const saveRoot = path.join(tmp, 'saves');
    await fs.mkdir(saveRoot, { recursive: true });
    await fs.writeFile(path.join(saveRoot, 'a.txt'), 'payload');

    const { revisionDir } = await packRevision(path.join(tmp, 'pack'), {
      gameId: 'game-a',
      saveLocations: [saveRoot],
      revisionId: '2026-02-01T00-00-00-000Z',
      createdAt: new Date('2026-02-01T00:00:00.000Z'),
    });

    const provider = new SelfHostStorageProvider({
      provider: 'selfhost',
      baseUrl,
      bearerToken: 'test-token',
    });

    const info = await provider.putRevision('game-a', revisionDir);
    assert.equal(info.revisionId, '2026-02-01T00-00-00-000Z');

    const listed = await provider.listRevisions('game-a');
    assert.equal(listed.length, 1);
    assert.equal(listed[0].contentHash, info.contentHash);

    const dest = path.join(tmp, 'out');
    const got = await provider.getRevision(
      'game-a',
      '2026-02-01T00-00-00-000Z',
      dest,
    );
    assert.equal(
      await fs.readFile(path.join(got, 'saves', path.basename(saveRoot), 'a.txt'), 'utf8'),
      'payload',
    );

    await provider.deleteRevision('game-a', '2026-02-01T00-00-00-000Z');
    assert.equal((await provider.listRevisions('game-a')).length, 0);

    await testSelfHostConnection({
      provider: 'selfhost',
      baseUrl,
      bearerToken: 'test-token',
    });

    await assert.rejects(
      () =>
        testSelfHostConnection({
          provider: 'selfhost',
          baseUrl,
          bearerToken: 'wrong',
        }),
      /auth failed/,
    );
  } finally {
    server.close();
    await fs.rm(tmp, { recursive: true, force: true });
  }
});

test('packTar/unpackTar round-trip', async () => {
  const tmp = await makeTemp();
  try {
    await fs.writeFile(path.join(tmp, 'f.txt'), 'x');
    const tar = await packTar(tmp);
    const dest = path.join(tmp, 'out');
    await unpackTar(tar, dest);
    assert.equal(await fs.readFile(path.join(dest, 'f.txt'), 'utf8'), 'x');
  } finally {
    await fs.rm(tmp, { recursive: true, force: true });
  }
});
