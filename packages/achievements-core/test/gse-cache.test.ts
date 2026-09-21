import { describe, test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-cache-'));
process.env.APPDATA = tmp;

const { ensureGseCache, GSE_ASSET_URL, GSE_ASSET_NAME, resolveLatestGseTag, gseAssetUrl } =
  await import('../dist/index.js');

function psQuote(s) {
  return `'${String(s).replace(/'/g, "''")}'`;
}

function zipFixture(entries) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-zip-'));
  const sources = [];
  for (const [rel, contents] of Object.entries(entries)) {
    const full = path.join(dir, rel);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, contents);
    sources.push(full);
  }
  const zipPath = path.join(dir, 'gse.zip');
  const list = sources.map(psQuote).join(',');
  execFileSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `Compress-Archive -LiteralPath @(${list}) -DestinationPath ${psQuote(zipPath)}`,
    ],
    { windowsHide: true },
  );
  return zipPath;
}

function stubFetchZip(zipPath, onFetch) {
  const orig = globalThis.fetch;
  globalThis.fetch = async (input) => {
    onFetch?.(String(input));
    return {
      ok: true,
      status: 200,
      arrayBuffer: async () => fs.readFileSync(zipPath),
    };
  };
  return () => {
    globalThis.fetch = orig;
  };
}

describe('ensureGseCache', { concurrency: false }, () => {
  test('default asset URL is the live emu-win-release.7z', () => {
    assert.match(GSE_ASSET_URL, /emu-win-release\.7z$/);
    assert.equal(GSE_ASSET_NAME, 'emu-win-release.7z');
  });

  test('gseAssetUrl uses tag + emu-win-release.7z', () => {
    assert.equal(
      gseAssetUrl('release-2099_01_01'),
      'https://github.com/Detanup01/gbe_fork/releases/download/release-2099_01_01/emu-win-release.7z',
    );
  });

  test('resolveLatestGseTag reads GitHub tag_name', async () => {
    const orig = globalThis.fetch;
    globalThis.fetch = async () => ({
      ok: true,
      status: 200,
      json: async () => ({ tag_name: 'release-2099_01_01' }),
    });
    try {
      assert.equal(await resolveLatestGseTag(), 'release-2099_01_01');
    } finally {
      globalThis.fetch = orig;
    }
  });

  test('miss downloads and extracts nested steam_api64.dll', async () => {
    const inner = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-nested-'));
    const folder = path.join(inner, 'emu-win-release');
    fs.mkdirSync(folder);
    fs.writeFileSync(path.join(folder, 'steam_api64.dll'), 'MZ-gse-fixture');
    const zipPath = path.join(inner, 'gse.zip');
    execFileSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `Compress-Archive -LiteralPath ${psQuote(folder)} -DestinationPath ${psQuote(zipPath)}`,
      ],
      { windowsHide: true },
    );

    let fetches = 0;
    const restore = stubFetchZip(zipPath, (url) => {
      fetches += 1;
      assert.equal(url, 'http://fixture.local/gse.zip');
    });
    try {
      const dest = path.join(tmp, 'miss');
      const dir = await ensureGseCache('http://fixture.local/gse.zip', dest);
      assert.ok(fs.existsSync(path.join(dir, 'steam_api64.dll')));
      assert.equal(fetches, 1);
    } finally {
      restore();
    }
  });

  test('second call does not fetch again', async () => {
    const dest = path.join(tmp, 'reuse');
    fs.mkdirSync(dest, { recursive: true });
    fs.writeFileSync(path.join(dest, 'steam_api64.dll'), 'MZ-cached');
    let fetches = 0;
    const orig = globalThis.fetch;
    globalThis.fetch = async () => {
      fetches += 1;
      return { ok: true, status: 200, arrayBuffer: async () => new Uint8Array() };
    };
    try {
      const dir = await ensureGseCache('http://example.invalid/gse.zip', dest);
      assert.equal(dir, dest);
      assert.equal(fetches, 0);
    } finally {
      globalThis.fetch = orig;
    }
  });

  test('HTTP failure throws a clear error', async () => {
    const orig = globalThis.fetch;
    globalThis.fetch = async () => ({
      ok: false,
      status: 404,
      arrayBuffer: async () => new Uint8Array(),
    });
    try {
      await assert.rejects(
        () => ensureGseCache('http://example.invalid/missing.zip', path.join(tmp, 'http-fail')),
        /404/,
      );
    } finally {
      globalThis.fetch = orig;
    }
  });

  test('zip without steam_api64.dll throws a clear error', async () => {
    const zipPath = zipFixture({ 'readme.txt': 'no dll here' });
    const restore = stubFetchZip(zipPath);
    try {
      await assert.rejects(
        () => ensureGseCache('http://fixture.local/gse.zip', path.join(tmp, 'no-dll')),
        /steam_api64\.dll/i,
      );
    } finally {
      restore();
    }
  });
});
