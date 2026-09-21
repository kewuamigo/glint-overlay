import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import {
  buildSessionSnapshot,
  sessionSnapshotPath,
  writeSessionSnapshot,
} from './session-snapshot.js';

const totals = { 'steam:hk': 45060 };

test('running game produces pid-keyed entry with accumulated seconds', () => {
  const snapshot = buildSessionSnapshot(
    [
      { id: 'steam:hk', name: 'Hollow Knight', pid: 1234, running: true },
      { id: 'steam:idle', name: 'Not Running', running: false },
      { id: 'custom:x', name: 'No Pid Yet', running: true },
    ],
    totals,
  );
  assert.deepEqual(snapshot, {
    games: [{ pid: 1234, name: 'Hollow Knight', totalSeconds: 45060 }],
  });
});

test('no games running → empty games list', () => {
  assert.deepEqual(
    buildSessionSnapshot(
      [{ id: 'steam:idle', name: 'Not Running', running: false }],
      totals,
    ),
    { games: [] },
  );
});

test('unknown game id snapshots zero seconds', () => {
  const snapshot = buildSessionSnapshot(
    [{ id: 'steam:new', name: 'New Game', pid: 9, running: true }],
    totals,
  );
  assert.equal(snapshot.games[0]?.totalSeconds, 0);
});

test('write lands at %APPDATA%/Glint/session.json and leaves no temp file', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-snapshot-'));
  writeSessionSnapshot(
    { games: [{ pid: 7, name: 'X', totalSeconds: 42 }] },
    root,
  );
  const raw = fs.readFileSync(sessionSnapshotPath(root), 'utf8');
  assert.deepEqual(JSON.parse(raw), {
    games: [{ pid: 7, name: 'X', totalSeconds: 42 }],
  });
  assert.ok(!fs.existsSync(`${sessionSnapshotPath(root)}.tmp`));
});
