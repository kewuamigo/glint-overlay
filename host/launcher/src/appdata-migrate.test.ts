import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import {
  migrateAppDataFromGameOverlay,
} from './appdata-migrate.js';
import {
  APP_DATA_DIR_NAME,
  LEGACY_APP_DATA_DIR_NAME,
  MIGRATION_MARKER,
} from './product.js';

test('copies legacy GameOverlay into Glint once', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-migrate-'));
  const legacy = path.join(root, LEGACY_APP_DATA_DIR_NAME);
  const glint = path.join(root, APP_DATA_DIR_NAME);
  fs.mkdirSync(path.join(legacy, 'apps', 'demo'), { recursive: true });
  fs.writeFileSync(path.join(legacy, 'apps', 'demo', 'x.txt'), 'ok');

  const first = migrateAppDataFromGameOverlay(root);
  assert.equal(first.migrated, true);
  assert.equal(first.reason, 'copied');
  assert.equal(
    fs.readFileSync(path.join(glint, 'apps', 'demo', 'x.txt'), 'utf8'),
    'ok',
  );
  assert.ok(fs.existsSync(path.join(glint, MIGRATION_MARKER)));

  fs.writeFileSync(path.join(legacy, 'apps', 'demo', 'x.txt'), 'changed');
  const second = migrateAppDataFromGameOverlay(root);
  assert.equal(second.migrated, false);
  assert.equal(second.reason, 'already-migrated');
  assert.equal(
    fs.readFileSync(path.join(glint, 'apps', 'demo', 'x.txt'), 'utf8'),
    'ok',
  );
});

test('no legacy: writes marker without inventing user data', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-migrate-'));
  const result = migrateAppDataFromGameOverlay(root);
  assert.equal(result.migrated, false);
  assert.equal(result.reason, 'no-legacy');
  assert.ok(
    fs.existsSync(path.join(root, APP_DATA_DIR_NAME, MIGRATION_MARKER)),
  );
});
