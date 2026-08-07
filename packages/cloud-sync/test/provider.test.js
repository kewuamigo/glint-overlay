import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { MemoryStorageProvider, packRevision } from '../dist/index.js';

async function makeTemp() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'go-cloud-prov-'));
}

test('MemoryStorageProvider put/get/list/deleteRevision', async () => {
  const tmp = await makeTemp();
  const provider = new MemoryStorageProvider(path.join(tmp, 'store'));

  const saveRoot = path.join(tmp, 'saves');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'a.txt'), '1');

  const { revisionDir } = await packRevision(path.join(tmp, 'pack'), {
    gameId: 'game-a',
    saveLocations: [saveRoot],
    achievements: { unlocked: ['x'] },
    revisionId: 'r1',
    createdAt: new Date('2026-02-01T00:00:00.000Z'),
  });

  const info = await provider.putRevision('game-a', revisionDir);
  assert.equal(info.revisionId, 'r1');
  assert.equal(info.gameId, 'game-a');

  const listed = await provider.listRevisions('game-a');
  assert.equal(listed.length, 1);
  assert.equal(listed[0].revisionId, 'r1');

  const dest = path.join(tmp, 'download');
  const got = await provider.getRevision('game-a', 'r1', dest);
  assert.ok(await fs.readFile(path.join(got, 'manifest.json'), 'utf8'));

  await provider.deleteRevision('game-a', 'r1');
  assert.equal((await provider.listRevisions('game-a')).length, 0);
});
