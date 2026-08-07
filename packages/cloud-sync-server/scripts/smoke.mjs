#!/usr/bin/env node
/**
 * Smoke: auth reject + upload/list/download/delete + long-path tar round-trip.
 */
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { packRevision } from '@glint/cloud-sync';
import { createCloudSyncServer } from '../dist/server.js';
import { packTar, unpackTar } from '../dist/tar.js';

const TOKEN = 'smoke-token';
const BAD = 'wrong-token';

function fail(msg) {
  console.error('FAIL:', msg);
  process.exit(1);
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const addr = server.address();
  return `http://127.0.0.1:${addr.port}`;
}

const root = await fs.mkdtemp(path.join(os.tmpdir(), 'go-css-smoke-'));
const packDir = await fs.mkdtemp(path.join(os.tmpdir(), 'go-css-pack-'));
const saveDir = path.join(packDir, 'saves-src');
await fs.mkdir(saveDir);
await fs.writeFile(path.join(saveDir, 'slot.sav'), 'hello-save');

const packed = await packRevision(path.join(packDir, 'out'), {
  gameId: 'demo-game',
  saveLocations: [saveDir],
  achievements: { unlocked: ['a1'] },
  revisionId: '2026-08-05T12-00-00-000Z',
  createdAt: new Date('2026-08-05T12:00:00.000Z'),
});

const server = createCloudSyncServer({ root, token: TOKEN });
const base = await listen(server);
console.log('server', base, 'root', root);

try {
  // 0) Long path >100 chars must round-trip (no silent truncation)
  {
    const longSrc = await fs.mkdtemp(path.join(os.tmpdir(), 'go-css-long-'));
    const longName = `${'a'.repeat(120)}.sav`;
    const payload = 'long-path-payload';
    await fs.writeFile(path.join(longSrc, longName), payload);
    const tarBuf = await packTar(longSrc);
    const longDest = path.join(packDir, 'long-unpacked');
    await unpackTar(tarBuf, longDest);
    const got = await fs.readFile(path.join(longDest, longName), 'utf8');
    if (got !== payload) fail(`long path content mismatch: ${got}`);
    await fs.rm(longSrc, { recursive: true, force: true });
    console.log('ok  long path >100 chars pack/unpack');
  }

  // 1) Unauthorized must not list / store
  {
    const res = await fetch(`${base}/v1/games/demo-game/revisions`);
    if (res.status !== 401) fail(`expected 401 without token, got ${res.status}`);
    const listing = await fs.readdir(root).catch(() => []);
    if (listing.length !== 0) fail('unauthorized list must not create storage');
    console.log('ok  unauthorized list → 401');
  }

  {
    const tar = await packTar(packed.revisionDir);
    const res = await fetch(
      `${base}/v1/games/demo-game/revisions/${packed.manifest.revisionId}`,
      {
        method: 'PUT',
        headers: {
          Authorization: `Bearer ${BAD}`,
          'Content-Type': 'application/x-tar',
        },
        body: tar,
      },
    );
    if (res.status !== 401) fail(`expected 401 bad token PUT, got ${res.status}`);
    const listing = await fs.readdir(root).catch(() => []);
    if (listing.length !== 0) fail('unauthorized PUT must not store blobs');
    console.log('ok  unauthorized PUT → 401, no store');
  }

  // 2) Upload + list + download
  {
    const tar = await packTar(packed.revisionDir);
    const res = await fetch(
      `${base}/v1/games/demo-game/revisions/${packed.manifest.revisionId}`,
      {
        method: 'PUT',
        headers: {
          Authorization: `Bearer ${TOKEN}`,
          'Content-Type': 'application/x-tar',
        },
        body: tar,
      },
    );
    if (res.status !== 201) fail(`PUT expected 201, got ${res.status} ${await res.text()}`);
    const info = await res.json();
    if (info.revisionId !== packed.manifest.revisionId) fail('PUT revisionId mismatch');
    console.log('ok  PUT revision → 201', info.revisionId);
  }

  {
    const res = await fetch(`${base}/v1/games/demo-game/revisions`, {
      headers: { Authorization: `Bearer ${TOKEN}` },
    });
    if (res.status !== 200) fail(`list expected 200, got ${res.status}`);
    const body = await res.json();
    if (!Array.isArray(body.revisions) || body.revisions.length !== 1) {
      fail(`list expected 1 revision, got ${JSON.stringify(body)}`);
    }
    console.log('ok  list → 1 revision');
  }

  {
    const res = await fetch(
      `${base}/v1/games/demo-game/revisions/${packed.manifest.revisionId}`,
      { headers: { Authorization: `Bearer ${TOKEN}` } },
    );
    if (res.status !== 200) fail(`GET expected 200, got ${res.status}`);
    const tar = Buffer.from(await res.arrayBuffer());
    const dest = path.join(packDir, 'downloaded');
    await unpackTar(tar, dest);
    const man = JSON.parse(
      await fs.readFile(path.join(dest, 'manifest.json'), 'utf8'),
    );
    if (man.gameId !== 'demo-game') fail('downloaded manifest gameId mismatch');
    console.log('ok  GET revision tar + unpack');
  }

  // 3) DELETE: unauthorized must not delete; authorized removes
  {
    const res = await fetch(
      `${base}/v1/games/demo-game/revisions/${packed.manifest.revisionId}`,
      {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${BAD}` },
      },
    );
    if (res.status !== 401) fail(`expected 401 bad token DELETE, got ${res.status}`);
    const still = await fs
      .access(path.join(root, 'demo-game', packed.manifest.revisionId, 'manifest.json'))
      .then(() => true)
      .catch(() => false);
    if (!still) fail('unauthorized DELETE must not remove revision');
    console.log('ok  unauthorized DELETE → 401, retained');
  }

  {
    const res = await fetch(
      `${base}/v1/games/demo-game/revisions/${packed.manifest.revisionId}`,
      {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${TOKEN}` },
      },
    );
    if (res.status !== 204) fail(`DELETE expected 204, got ${res.status} ${await res.text()}`);
    const gone = await fs
      .access(path.join(root, 'demo-game', packed.manifest.revisionId, 'manifest.json'))
      .then(() => false)
      .catch(() => true);
    if (!gone) fail('authorized DELETE must remove revision');
    const listRes = await fetch(`${base}/v1/games/demo-game/revisions`, {
      headers: { Authorization: `Bearer ${TOKEN}` },
    });
    const body = await listRes.json();
    if (!Array.isArray(body.revisions) || body.revisions.length !== 0) {
      fail(`list after DELETE expected 0, got ${JSON.stringify(body)}`);
    }
    console.log('ok  DELETE revision → 204, list empty');
  }

  console.log('SMOKE PASS');
} finally {
  await new Promise((r) => server.close(r));
  await fs.rm(root, { recursive: true, force: true });
  await fs.rm(packDir, { recursive: true, force: true });
}
