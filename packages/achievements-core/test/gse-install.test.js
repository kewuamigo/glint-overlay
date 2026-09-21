import { describe, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-install-'));
process.env.APPDATA = tmp;

const { ensureGseForGame } = await import('../dist/index.js');

function writeFile(filePath, contents) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, contents);
}

function listRel(root) {
  const out = [];
  for (const name of fs.readdirSync(root, { recursive: true }).sort()) {
    const full = path.join(root, name);
    if (fs.statSync(full).isFile()) out.push(String(name).replaceAll('\\', '/'));
  }
  return out;
}

function fixtureCache() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-cache-fx-'));
  writeFile(path.join(dir, 'steam_api64.dll'), 'MZ-FROM-CACHE');
  writeFile(path.join(dir, 'steam_api.dll'), 'MZ-FROM-CACHE-32');
  return dir;
}

describe('ensureGseForGame', { concurrency: false }, () => {
  test("source: 'steam' leaves game files unchanged", async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-steam-src-'));
    const exe = path.join(gameDir, 'game.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
    const before = listRel(gameDir);

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'steam',
      appId: '480',
      cacheDir: fixtureCache(),
    });

    assert.equal(result.installed, false);
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'), 'MZ-ORIGINAL');
    assert.deepEqual(listRel(gameDir), before);
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_api64.dll.bak')), false);
  });

  test('already has steam_settings + GSE-marked dll → dll not replaced', async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-already-'));
    const exe = path.join(gameDir, 'game.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-gbe_fork-EXISTING');
    fs.mkdirSync(path.join(gameDir, 'steam_settings'));

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'custom',
      appId: '3321460',
      cacheDir: fixtureCache(),
    });

    assert.equal(result.installed, false);
    assert.equal(result.gameDir, gameDir);
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'),
      'MZ-gbe_fork-EXISTING',
    );
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_api64.dll.bak')), false);
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3321460',
    );
  });

  test('plain steam_api64.dll custom → bak, cache dll, steam_appid.txt', async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-install-custom-'));
    const exe = path.join(gameDir, 'game.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
    const cacheDir = fixtureCache();

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'custom',
      appId: '3321460',
      cacheDir,
    });

    assert.equal(result.installed, true);
    assert.equal(result.gameDir, gameDir);
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api64.dll.bak'), 'utf8'), 'MZ-ORIGINAL');
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'), 'MZ-FROM-CACHE');
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3321460',
    );
  });

  test('32-bit-only game copies Win32 dll, not x64', async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-win32-only-'));
    const exe = path.join(gameDir, 'game.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'steam_api.dll'), 'MZ-ORIGINAL-32');

    const regular = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-regular-'));
    const cacheDir = path.join(regular, 'x64');
    writeFile(path.join(cacheDir, 'steam_api64.dll'), 'MZ-FROM-CACHE');
    writeFile(path.join(regular, 'Win32', 'steam_api.dll'), 'MZ-FROM-WIN32');

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'custom',
      appId: '3321460',
      cacheDir,
    });

    assert.equal(result.installed, true);
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api.dll.bak'), 'utf8'), 'MZ-ORIGINAL-32');
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api.dll'), 'utf8'), 'MZ-FROM-WIN32');
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_api64.dll')), false);
  });

  test('unreal Steamworks Win64 dll is patched, not the shipping exe folder', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-ue-'));
    const exe = path.join(root, 'Game', 'Binaries', 'Win64', 'game.exe');
    const steamDir = path.join(
      root,
      'Engine',
      'Binaries',
      'Thirdparty',
      'Steamworks',
      'Steamv157',
      'Win64',
    );
    writeFile(exe, '');
    writeFile(path.join(steamDir, 'steam_api64.dll'), 'MZ-ORIGINAL');

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'custom',
      appId: '3751260',
      cacheDir: fixtureCache(),
    });

    assert.equal(result.installed, true);
    assert.equal(path.resolve(result.gameDir).toLowerCase(), path.resolve(steamDir).toLowerCase());
    assert.equal(fs.readFileSync(path.join(steamDir, 'steam_api64.dll.bak'), 'utf8'), 'MZ-ORIGINAL');
    assert.equal(fs.readFileSync(path.join(steamDir, 'steam_api64.dll'), 'utf8'), 'MZ-FROM-CACHE');
    assert.equal(
      fs.readFileSync(path.join(steamDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3751260',
    );
    assert.equal(fs.existsSync(path.join(path.dirname(exe), 'steam_api64.dll')), false);
  });

  test('force installs steam_api64 when folder has no Steamworks dll', async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-force-nodll-'));
    const exe = path.join(gameDir, 'ACShadows.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-UPC');

    const result = await ensureGseForGame({
      exePath: exe,
      source: 'custom',
      appId: '3159330',
      cacheDir: fixtureCache(),
      force: true,
    });

    assert.equal(result.installed, true);
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'), 'MZ-FROM-CACHE');
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3159330',
    );
    assert.equal(fs.existsSync(path.join(gameDir, 'upc_r2_loader64.dll')), true);
  });

  test('missing appId when install required → throw', async () => {
    const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-gse-no-appid-'));
    const exe = path.join(gameDir, 'game.exe');
    writeFile(exe, '');
    writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');

    await assert.rejects(
      () =>
        ensureGseForGame({
          exePath: exe,
          source: 'custom',
          cacheDir: fixtureCache(),
        }),
      /appid/i,
    );
    assert.equal(fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'), 'MZ-ORIGINAL');
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_api64.dll.bak')), false);
  });
});
