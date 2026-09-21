import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  detectExe,
  findGameExeInFolder,
  gameRootFromExe,
  resolveSteamAppId,
} from '../dist/index.js';

async function makeTemp() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'go-ach-detect-'));
}

async function writeFile(filePath, contents) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, contents);
}

test('bin64 exe with steam_settings and steam_api64.dll is watchable GSE', async () => {
  const tmp = await makeTemp();
  const bin64 = path.join(tmp, 'bin64');
  const exe = path.join(bin64, 'CrimsonDesert.exe');
  await writeFile(exe, '');
  await fs.mkdir(path.join(bin64, 'steam_settings'));
  await writeFile(path.join(bin64, 'steam_api64.dll'), 'MZ');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
});

test('bin64 exe finds steam_api64.dll and steam_settings in parent', async () => {
  const tmp = await makeTemp();
  const bin64 = path.join(tmp, 'bin64');
  const exe = path.join(bin64, 'game.exe');
  await writeFile(exe, '');
  await fs.mkdir(path.join(tmp, 'steam_settings'));
  await writeFile(path.join(tmp, 'steam_api64.dll'), 'MZ');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
});

test('steam_api64.dll with gbe_fork bytes is watchable GSE without steam_settings', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(
    path.join(tmp, 'steam_api64.dll'),
    'MZ....gbe_fork....GSE Client API....',
  );

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
});

test('plain steam_api64.dll without GSE markers is steam but not supported', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(path.join(tmp, 'steam_api64.dll'), 'MZ steamworks');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, false);
  assert.ok(!result.markers.includes('gse'));
  assert.ok(!result.markers.includes('uplay'));
});

test('steam_api plus upc_r2_loader64.dll is steam with uplay marker', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(path.join(tmp, 'steam_api64.dll'), 'MZ steamworks');
  await writeFile(path.join(tmp, 'upc_r2_loader64.dll'), 'MZ');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.ok(result.markers.includes('uplay'));
});

test('watchable GSE plus upc_r2.ini stays steam supported with uplay', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(path.join(tmp, 'steam_api64.dll'), 'MZ-gbe_fork');
  await fs.mkdir(path.join(tmp, 'steam_settings'));
  await writeFile(path.join(tmp, 'upc_r2.ini'), '[Settings]\nAchievements = 0\n');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
  assert.ok(result.markers.includes('uplay'));
});

test('unreal Engine/Binaries/Thirdparty/Steamworks is detected from Win64 shipping exe', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'Dawnwalker', 'Binaries', 'Win64', 'Dawnwalker-Win64-Shipping.exe');
  const steamDir = path.join(
    tmp,
    'Engine',
    'Binaries',
    'Thirdparty',
    'Steamworks',
    'Steamv157',
    'Win64',
  );
  await writeFile(exe, '');
  await writeFile(path.join(steamDir, 'steam_api64.dll'), 'MZ....gbe_fork....');
  await fs.mkdir(path.join(steamDir, 'steam_settings'));

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
});

test('unreal Steamworks next to install-root exe is detected', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'Dawnwalker.exe');
  const steamDir = path.join(
    tmp,
    'Engine',
    'Binaries',
    'ThirdParty',
    'Steamworks',
    'Steamv157',
    'Win64',
  );
  await writeFile(exe, '');
  await writeFile(path.join(steamDir, 'steam_api64.dll'), 'MZ steamworks');

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, false);
  assert.ok(result.markers.includes('steam_api'));
});

test('unreal Steamworks steam_appid.txt is resolved from shipping exe', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'Game', 'Binaries', 'Win64', 'game.exe');
  const steamDir = path.join(
    tmp,
    'Engine',
    'Binaries',
    'Thirdparty',
    'Steamworks',
    'Steamv157',
    'Win64',
  );
  await writeFile(exe, '');
  await writeFile(path.join(steamDir, 'steam_settings', 'steam_appid.txt'), '3751260\n');

  assert.equal(resolveSteamAppId(exe), '3751260');
});

test('gameRootFromExe walks Binaries/Win64 up to the install folder', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'The Blood of Dawnwalker', 'Binaries', 'Win64', 'Dawnwalker.exe');
  await writeFile(exe, '');
  assert.equal(gameRootFromExe(exe), path.join(tmp, 'The Blood of Dawnwalker'));
});

test('steam_api under an arbitrary game-root subfolder is detected', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'The Blood of Dawnwalker', 'Binaries', 'Win64', 'Dawnwalker.exe');
  await writeFile(exe, '');
  await writeFile(
    path.join(tmp, 'The Blood of Dawnwalker', 'redist', 'steam', 'steam_api64.dll'),
    'MZ....gbe_fork....',
  );

  const result = detectExe(exe);
  assert.equal(result.platform, 'steam');
  assert.equal(result.supported, true);
  assert.ok(result.markers.includes('gse'));
});

test('findGameExeInFolder prefers Binaries/Win64 shipping over helpers', async () => {
  const tmp = await makeTemp();
  const shipping = path.join(tmp, 'Binaries', 'Win64', 'Dawnwalker-Win64-Shipping.exe');
  await writeFile(shipping, '');
  await writeFile(path.join(tmp, 'Engine', 'Binaries', 'Win64', 'UnrealInsights.exe'), '');
  await writeFile(path.join(tmp, 'EasyAntiCheat', 'EasyAntiCheat_EOS_Setup.exe'), '');
  await writeFile(path.join(tmp, 'unins000.exe'), '');

  assert.equal(findGameExeInFolder(tmp), shipping);
});

test('does not walk parent when exe folder is not bin/bin64/binaries/win64', async () => {
  const tmp = await makeTemp();
  const gameDir = path.join(tmp, 'MyGame');
  const exe = path.join(gameDir, 'game.exe');
  await writeFile(exe, '');
  await fs.mkdir(path.join(tmp, 'steam_settings'));
  await writeFile(path.join(tmp, 'steam_api64.dll'), 'MZ');

  const result = detectExe(exe);
  assert.equal(result.platform, 'unknown');
  assert.equal(result.supported, false);
  assert.ok(!result.markers.includes('gse'));
});

test('bin64 steam_appid.txt resolves 3321460', async () => {
  const tmp = await makeTemp();
  const bin64 = path.join(tmp, 'bin64');
  const exe = path.join(bin64, 'CrimsonDesert.exe');
  await writeFile(exe, '');
  await writeFile(path.join(bin64, 'steam_appid.txt'), '3321460\n');

  assert.equal(resolveSteamAppId(exe), '3321460');
});

test('bin64 exe finds AppID in parent steam_settings/steam_appid.txt', async () => {
  const tmp = await makeTemp();
  const bin64 = path.join(tmp, 'bin64');
  const exe = path.join(bin64, 'game.exe');
  await writeFile(exe, '');
  await writeFile(path.join(tmp, 'steam_settings', 'steam_appid.txt'), '3321460');

  assert.equal(resolveSteamAppId(exe), '3321460');
});

test('steam_api.ini AppId=123 is used when steam_appid.txt is missing', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(path.join(tmp, 'steam_api.ini'), 'AppId=123\n');

  assert.equal(resolveSteamAppId(exe), '123');
});

test('returns null when no AppID files exist', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');

  assert.equal(resolveSteamAppId(exe), null);
});

test('does not treat configs.app.ini [app::dlcs] ids as the game AppID', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(
    path.join(tmp, 'configs.app.ini'),
    '[app::dlcs]\nunlock_all=1\n4024620 = Deluxe Pack\n',
  );

  assert.equal(resolveSteamAppId(exe), null);
});

test('configs.app.ini [app::general] appid= wins over DLC list keys', async () => {
  const tmp = await makeTemp();
  const exe = path.join(tmp, 'game.exe');
  await writeFile(exe, '');
  await writeFile(
    path.join(tmp, 'configs.app.ini'),
    '[app::general]\nappid=3321460\n\n[app::dlcs]\n4024620 = Deluxe Pack\n',
  );

  assert.equal(resolveSteamAppId(exe), '3321460');
});

test('bin64 exe finds AppID in parent steam_settings/configs.app.ini', async () => {
  const tmp = await makeTemp();
  const bin64 = path.join(tmp, 'bin64');
  const exe = path.join(bin64, 'game.exe');
  await writeFile(exe, '');
  await writeFile(
    path.join(tmp, 'steam_settings', 'configs.app.ini'),
    '[app::general]\nappid=3321460\n',
  );

  assert.equal(resolveSteamAppId(exe), '3321460');
});
