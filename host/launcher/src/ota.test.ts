import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, it } from 'node:test';
import {
  checksumAssetName,
  parseSha256File,
  pickReleaseAssets,
  portableAssetName,
  setupAssetName,
  sha256File,
} from './ota-assets.js';
import { detectInstallLayout, innoUninstallKey } from './ota-layout.js';
import {
  compareSemver,
  isNewerVersion,
  parseSemver,
  stripV,
} from './ota-semver.js';
import { DEV_VERSION_FALLBACK, readProductVersion } from './ota-version.js';
import { INNO_APP_ID } from './product.js';

describe('semver compare', () => {
  it('strips a leading v', () => {
    assert.equal(stripV('v0.2.0'), '0.2.0');
    assert.equal(stripV('V1.0.0'), '1.0.0');
    assert.equal(stripV('0.2.0'), '0.2.0');
  });

  it('parses major.minor.patch and optional prerelease', () => {
    assert.deepEqual(parseSemver('v0.2.0'), {
      major: 0,
      minor: 2,
      patch: 0,
      prerelease: null,
    });
    assert.deepEqual(parseSemver('1.2.3-beta.1'), {
      major: 1,
      minor: 2,
      patch: 3,
      prerelease: 'beta.1',
    });
    assert.equal(parseSemver('not-a-version'), null);
    assert.equal(parseSemver('1.0'), null);
  });

  it('treats a higher tag as newer (0.1.0 vs 0.2.0)', () => {
    assert.equal(isNewerVersion('0.1.0', '0.2.0'), true);
    assert.equal(isNewerVersion('0.1.0', 'v0.2.0'), true);
    assert.equal(isNewerVersion('v0.2.0', '0.2.0'), false);
    assert.equal(isNewerVersion('0.2.0', '0.1.9'), false);
    assert.equal(isNewerVersion('1.0.0', '0.9.9'), false);
    assert.equal(isNewerVersion('0.0.0-dev', '0.1.0'), true);
  });

  it('orders prerelease below the matching release', () => {
    const a = parseSemver('1.0.0-rc.1');
    const b = parseSemver('1.0.0');
    assert.ok(a && b);
    assert.ok(compareSemver(a, b) < 0);
    assert.ok(compareSemver(b, a) > 0);
  });
});

describe('asset name selection', () => {
  const assets = [
    {
      name: 'Glint-Setup-0.2.0.exe',
      browser_download_url: 'https://example.test/setup',
    },
    {
      name: 'Glint-Setup-0.2.0.exe.sha256',
      browser_download_url: 'https://example.test/sha',
    },
    {
      name: 'Glint-portable-0.2.0.zip',
      browser_download_url: 'https://example.test/zip',
    },
    {
      name: 'notes.md',
      browser_download_url: 'https://example.test/notes',
    },
  ];

  it('names Setup / checksum / portable from the release version', () => {
    assert.equal(setupAssetName('v0.2.0'), 'Glint-Setup-0.2.0.exe');
    assert.equal(checksumAssetName('0.2.0'), 'Glint-Setup-0.2.0.exe.sha256');
    assert.equal(portableAssetName('0.2.0'), 'Glint-portable-0.2.0.zip');
  });

  it('picks exact Setup / checksum / portable names', () => {
    const picked = pickReleaseAssets(assets, 'v0.2.0');
    assert.equal(picked.setup?.browser_download_url, 'https://example.test/setup');
    assert.equal(picked.checksum?.browser_download_url, 'https://example.test/sha');
    assert.equal(picked.portable?.browser_download_url, 'https://example.test/zip');
  });

  it('does not invent a Setup asset when the exe is missing', () => {
    const picked = pickReleaseAssets(
      assets.filter((a) => a.name !== 'Glint-Setup-0.2.0.exe'),
      '0.2.0',
    );
    assert.equal(picked.setup, null);
    assert.ok(picked.portable);
  });

  it('does not pick a different version Setup as the current one', () => {
    const picked = pickReleaseAssets(
      [
        {
          name: 'Glint-Setup-0.1.0.exe',
          browser_download_url: 'https://example.test/old',
        },
        {
          name: 'Glint-portable-0.2.0.zip',
          browser_download_url: 'https://example.test/zip',
        },
      ],
      '0.2.0',
    );
    assert.equal(picked.setup, null);
  });
});

describe('checksum verify', () => {
  it('parses GNU sidecar, SHA256SUMS listing, and bare hex', () => {
    const name = 'Glint-Setup-0.2.0.exe';
    const hex = 'a'.repeat(64);
    assert.equal(parseSha256File(`${hex}  ${name}\n`, name), hex);
    assert.equal(parseSha256File(`${hex} *${name}\n`, name), hex);
    assert.equal(
      parseSha256File(
        `${'b'.repeat(64)}  other.exe\n${hex}  ${name}\n`,
        name,
      ),
      hex,
    );
    assert.equal(parseSha256File(`${hex}\n`, name), hex);
    assert.equal(parseSha256File('not-a-hash\n', name), null);
    assert.equal(
      parseSha256File(`${'a'.repeat(64)}\n${'b'.repeat(64)}\n`, name),
      null,
    );
  });

  it('hashes a fixture file and matches the sidecar', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-ota-'));
    const file = path.join(dir, 'Glint-Setup-0.2.0.exe');
    const payload = Buffer.from('glint-setup-fixture\n');
    fs.writeFileSync(file, payload);
    const digest = createHash('sha256').update(payload).digest('hex');
    const sidecar = `${digest}  Glint-Setup-0.2.0.exe\n`;
    assert.equal(await sha256File(file), digest);
    assert.equal(parseSha256File(sidecar, 'Glint-Setup-0.2.0.exe'), digest);
    assert.notEqual(parseSha256File(sidecar, 'Glint-Setup-0.2.0.exe'), '0'.repeat(64));
  });
});

describe('install layout', () => {
  it('uses the Inno AppId uninstall key', () => {
    assert.equal(innoUninstallKey(), `${INNO_APP_ID}_is1`);
  });

  it('treats unins000.exe beside the root as Inno', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-inno-'));
    fs.writeFileSync(path.join(dir, 'unins000.exe'), '');
    assert.equal(
      detectInstallLayout(dir, { registryInstallLocation: null }),
      'inno',
    );
  });

  it('treats a bare extract as portable when registry is empty', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-port-'));
    assert.equal(
      detectInstallLayout(dir, { registryInstallLocation: null }),
      'portable',
    );
  });

  it('treats registry InstallLocation matching this tree as Inno', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-reg-'));
    assert.equal(
      detectInstallLayout(dir, { registryInstallLocation: dir }),
      'inno',
    );
  });

  it('keeps a zip extract portable even if another Setup install exists', () => {
    const portable = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-zip-'));
    const installed = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-pf-'));
    assert.equal(
      detectInstallLayout(portable, { registryInstallLocation: installed }),
      'portable',
    );
  });
});

describe('product version bake', () => {
  it('reads packaged version.json from the install root', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-ver-'));
    fs.writeFileSync(
      path.join(dir, 'version.json'),
      JSON.stringify({ version: '0.2.0' }),
    );
    assert.equal(readProductVersion(dir), '0.2.0');
  });

  it('falls back when version.json is absent', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'glint-ver-missing-'));
    const ver = readProductVersion(dir);
    assert.ok(ver === '0.1.0' || ver === DEV_VERSION_FALLBACK);
  });
});
