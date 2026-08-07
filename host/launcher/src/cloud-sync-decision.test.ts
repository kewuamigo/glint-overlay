import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  decideUpload,
  pickLatestRevision,
  remoteIsNewerThanCursor,
  unwrapAchievementRows,
} from './cloud-sync-decision.js';

describe('remoteIsNewerThanCursor', () => {
  it('false when no remote', () => {
    assert.equal(
      remoteIsNewerThanCursor({
        remoteCreatedAt: null,
        lastSuccessfulSync: '2024-01-01T00:00:00.000Z',
      }),
      false,
    );
  });

  it('true when remote exists and device has no cursor', () => {
    assert.equal(
      remoteIsNewerThanCursor({
        remoteCreatedAt: '2024-06-01T00:00:00.000Z',
        lastSuccessfulSync: null,
      }),
      true,
    );
  });

  it('true when remote createdAt is after cursor', () => {
    assert.equal(
      remoteIsNewerThanCursor({
        remoteCreatedAt: '2024-06-02T00:00:00.000Z',
        lastSuccessfulSync: '2024-06-01T00:00:00.000Z',
      }),
      true,
    );
  });

  it('false when remote equals or is older than cursor', () => {
    assert.equal(
      remoteIsNewerThanCursor({
        remoteCreatedAt: '2024-06-01T00:00:00.000Z',
        lastSuccessfulSync: '2024-06-01T00:00:00.000Z',
      }),
      false,
    );
    assert.equal(
      remoteIsNewerThanCursor({
        remoteCreatedAt: '2024-05-01T00:00:00.000Z',
        lastSuccessfulSync: '2024-06-01T00:00:00.000Z',
      }),
      false,
    );
  });
});

describe('decideUpload', () => {
  it('uploads when remote is not newer', () => {
    assert.equal(
      decideUpload({ remoteNewer: false, unattended: true }),
      'upload',
    );
  });

  it('skips unattended when remote newer (D8)', () => {
    assert.equal(
      decideUpload({ remoteNewer: true, unattended: true }),
      'skip-conflict',
    );
  });

  it('manual overwrite forces upload', () => {
    assert.equal(
      decideUpload({
        remoteNewer: true,
        unattended: false,
        overwrite: true,
      }),
      'upload',
    );
  });

  it('keepRemote wins without upload', () => {
    assert.equal(
      decideUpload({
        remoteNewer: true,
        unattended: false,
        keepRemote: true,
      }),
      'keep-remote',
    );
  });

  it('manual without flags surfaces conflict', () => {
    assert.equal(
      decideUpload({ remoteNewer: true, unattended: false }),
      'skip-conflict',
    );
  });
});

describe('pickLatestRevision', () => {
  it('returns null for empty', () => {
    assert.equal(pickLatestRevision([]), null);
  });

  it('picks max createdAt', () => {
    const latest = pickLatestRevision([
      { createdAt: '2024-01-01T00:00:00.000Z', id: 'a' },
      { createdAt: '2024-03-01T00:00:00.000Z', id: 'b' },
      { createdAt: '2024-02-01T00:00:00.000Z', id: 'c' },
    ]);
    assert.equal(latest?.id, 'b');
  });
});

describe('unwrapAchievementRows', () => {
  it('accepts a bare array', () => {
    const rows = [{ achievement_id: 'a1' }];
    assert.equal(unwrapAchievementRows(rows), rows);
  });

  it('unwraps { achievements: [...] } pack shape', () => {
    const rows = [{ achievement_id: 'a1' }];
    assert.deepEqual(
      unwrapAchievementRows({ gameId: 'g', achievements: rows }),
      rows,
    );
  });

  it('returns null for missing / invalid', () => {
    assert.equal(unwrapAchievementRows(null), null);
    assert.equal(unwrapAchievementRows({}), null);
    assert.equal(unwrapAchievementRows({ achievements: 'x' }), null);
  });
});
