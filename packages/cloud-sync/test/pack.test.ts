import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  packRevision,
  unpackRevision,
  readRevisionManifest,
} from '../dist/index.js';

async function makeTemp() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'go-cloud-pack-'));
}

test('pack saves + achievements together', async () => {
  const tmp = await makeTemp();
  const saveRoot = path.join(tmp, 'game-saves');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'save.dat'), 'slot1');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'game-1',
    saveLocations: [saveRoot],
    achievements: { unlocked: ['a1'] },
    revisionId: 'rev-both',
    createdAt: new Date('2026-01-01T00:00:00.000Z'),
  });

  assert.equal(manifest.gameId, 'game-1');
  assert.equal(manifest.revisionId, 'rev-both');
  assert.ok(manifest.saves.contentHash);
  assert.ok(manifest.achievements.contentHash);
  assert.equal(manifest.achievements.path, 'achievements.json');

  const ach = JSON.parse(
    await fs.readFile(path.join(revisionDir, 'achievements.json'), 'utf8'),
  );
  assert.deepEqual(ach, { unlocked: ['a1'] });

  const packed = await fs.readFile(
    path.join(revisionDir, 'saves', path.basename(saveRoot), 'save.dat'),
    'utf8',
  );
  assert.equal(packed, 'slot1');
});

test('pack saves-only when achievements omitted', async () => {
  const tmp = await makeTemp();
  const saveRoot = path.join(tmp, 'saves-only');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'f.txt'), 'x');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g2',
    saveLocations: [saveRoot],
    revisionId: 'rev-saves',
  });

  assert.ok(manifest.saves.contentHash);
  assert.equal(manifest.achievements.path, null);
  assert.equal(manifest.achievements.contentHash, null);
  await assert.rejects(
    () => fs.access(path.join(revisionDir, 'achievements.json')),
  );
});

test('pack achievements-only when save roots missing/empty', async () => {
  const tmp = await makeTemp();
  const missing = path.join(tmp, 'does-not-exist');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g3',
    saveLocations: [missing],
    achievements: { unlocked: [] },
    revisionId: 'rev-ach',
  });

  assert.equal(manifest.saves.roots.length, 1);
  assert.equal(manifest.saves.roots[0].contentHash, null);
  assert.equal(manifest.saves.contentHash, null);
  assert.ok(manifest.achievements.contentHash);
  assert.ok(await fs.readFile(path.join(revisionDir, 'achievements.json'), 'utf8'));
});

test('pack empty payload (no saves, no achievements) still succeeds', async () => {
  const tmp = await makeTemp();
  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g4',
    revisionId: 'rev-empty',
  });

  assert.equal(manifest.saves.roots.length, 0);
  assert.equal(manifest.saves.contentHash, null);
  assert.equal(manifest.achievements.path, null);
  assert.ok(manifest.contentHash);
  await readRevisionManifest(revisionDir);
});

test('unpack restores saves and returns achievements', async () => {
  const tmp = await makeTemp();
  const saveRoot = path.join(tmp, 'orig');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'slot.bin'), 'data');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g5',
    saveLocations: [saveRoot],
    achievements: { unlocked: ['boss'] },
    revisionId: 'rev-unpack',
  });

  const restoreDest = path.join(tmp, 'restored');
  const rootName = manifest.saves.roots[0].name;
  const result = await unpackRevision(revisionDir, {
    saveTargets: { [rootName]: restoreDest },
  });

  assert.deepEqual(result.achievements, { unlocked: ['boss'] });
  assert.equal(
    await fs.readFile(path.join(restoreDest, 'slot.bin'), 'utf8'),
    'data',
  );
});

test('file-root pack + unpack round-trip', async () => {
  const tmp = await makeTemp();
  const saveFile = path.join(tmp, 'single.sav');
  await fs.writeFile(saveFile, 'payload-bytes');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g-file',
    saveLocations: [saveFile],
    revisionId: 'rev-file',
  });

  const root = manifest.saves.roots[0];
  assert.ok(root.contentHash, 'file root contentHash must be non-null');
  const packed = path.join(revisionDir, 'saves', root.name);
  assert.equal((await fs.stat(packed)).isFile(), true);
  assert.equal(await fs.readFile(packed, 'utf8'), 'payload-bytes');

  const restoreDest = path.join(tmp, 'restored.sav');
  await unpackRevision(revisionDir, {
    saveTargets: { [root.name]: restoreDest },
  });
  assert.equal((await fs.stat(restoreDest)).isFile(), true);
  assert.equal(await fs.readFile(restoreDest, 'utf8'), 'payload-bytes');
});

test('directory restore removes orphan local files', async () => {
  const tmp = await makeTemp();
  const saveRoot = path.join(tmp, 'orig');
  await fs.mkdir(saveRoot, { recursive: true });
  await fs.writeFile(path.join(saveRoot, 'keep.dat'), 'keep');

  const { revisionDir, manifest } = await packRevision(path.join(tmp, 'out'), {
    gameId: 'g-orphan',
    saveLocations: [saveRoot],
    revisionId: 'rev-orphan',
  });

  const restoreDest = path.join(tmp, 'restored');
  await fs.mkdir(restoreDest, { recursive: true });
  await fs.writeFile(path.join(restoreDest, 'keep.dat'), 'stale');
  await fs.writeFile(path.join(restoreDest, 'orphan.dat'), 'local-only');

  const rootName = manifest.saves.roots[0].name;
  await unpackRevision(revisionDir, {
    saveTargets: { [rootName]: restoreDest },
  });

  assert.equal(await fs.readFile(path.join(restoreDest, 'keep.dat'), 'utf8'), 'keep');
  await assert.rejects(() => fs.access(path.join(restoreDest, 'orphan.dat')));
});
