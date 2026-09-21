import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'go-upc-bridge-'));
process.env.APPDATA = tmp;

const {
  mapUpcInId,
  mergeGseUnlock,
  gseUnlockPath,
  bridgeUpcUnlock,
  enableUpcForGame,
  ensureGoldbergUpcForGame,
} = await import('../dist/index.js');

const NAMES = ['ACObsidian_Ach_1', 'ACObsidian_Ach_2', 'ACObsidian_Ach_3'];

test('mapUpcInId exact schema name', () => {
  assert.equal(mapUpcInId('ACObsidian_Ach_2', NAMES), 'ACObsidian_Ach_2');
});

test('mapUpcInId 1-based then 0-based numeric index', () => {
  assert.equal(mapUpcInId('2', NAMES), 'ACObsidian_Ach_2');
  assert.equal(mapUpcInId('0', NAMES), 'ACObsidian_Ach_1');
});

test('mapUpcInId unmapped returns null', () => {
  assert.equal(mapUpcInId('99', NAMES), null);
  assert.equal(mapUpcInId('nope', NAMES), null);
  assert.equal(mapUpcInId('2', []), null);
});

test('mergeGseUnlock keeps other keys', () => {
  const file = path.join(tmp, 'merge.json');
  fs.writeFileSync(
    file,
    JSON.stringify({
      ACObsidian_Ach_1: { earned: true, earned_time: 1 },
    }),
  );
  mergeGseUnlock(file, 'ACObsidian_Ach_2', 99);
  const out = JSON.parse(fs.readFileSync(file, 'utf8'));
  assert.equal(out.ACObsidian_Ach_1.earned, true);
  assert.equal(out.ACObsidian_Ach_1.earned_time, 1);
  assert.equal(out.ACObsidian_Ach_2.earned, true);
  assert.equal(out.ACObsidian_Ach_2.earned_time, 99);
});

test('bridgeUpcUnlock writes GSE Saves under resolved AppID', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'game-'));
  const exe = path.join(gameDir, 'game.exe');
  fs.writeFileSync(exe, '');
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  fs.writeFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), '3751950\n');
  fs.writeFileSync(
    path.join(gameDir, 'steam_settings', 'achievements.json'),
    JSON.stringify(NAMES.map((name) => ({ name }))),
  );
  const mapped = bridgeUpcUnlock({
    exePath: exe,
    inId: '2',
    earnedTimeSec: 42,
    appData: tmp,
  });
  assert.equal(mapped, 'ACObsidian_Ach_2');
  const dest = gseUnlockPath('3751950', tmp);
  const out = JSON.parse(fs.readFileSync(dest, 'utf8'));
  assert.equal(out.ACObsidian_Ach_2.earned, true);
  assert.equal(out.ACObsidian_Ach_2.earned_time, 42);
});

test('bridgeUpcUnlock drops unmapped inId', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'game2-'));
  const exe = path.join(gameDir, 'game.exe');
  fs.writeFileSync(exe, '');
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  fs.writeFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), '3751950\n');
  fs.writeFileSync(
    path.join(gameDir, 'steam_settings', 'achievements.json'),
    JSON.stringify(NAMES.map((name) => ({ name }))),
  );
  const dest = gseUnlockPath('3751950', tmp);
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(
    dest,
    JSON.stringify({ ACObsidian_Ach_1: { earned: true, earned_time: 1 } }),
  );
  assert.equal(bridgeUpcUnlock({ exePath: exe, inId: 'nope', appData: tmp }), null);
  const out = JSON.parse(fs.readFileSync(dest, 'utf8'));
  assert.deepEqual(out, { ACObsidian_Ach_1: { earned: true, earned_time: 1 } });
});

test('enableUpcForGame sets ini and writes product achievements.json', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'upc-en-'));
  const exe = path.join(gameDir, 'ACBlackFlag.exe');
  fs.writeFileSync(exe, '');
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  fs.writeFileSync(
    path.join(gameDir, 'steam_settings', 'achievements.json'),
    JSON.stringify([{ name: 'ACObsidian_Ach_33', displayName: 'Yo Ho', description: 'shanty' }]),
  );
  fs.writeFileSync(path.join(gameDir, 'upc_r2.ini'), '[Settings]\nAchievements = 0\n');
  const product = path.join(tmp, 'Goldberg UplayEmu Saves', '66088');
  fs.mkdirSync(product, { recursive: true });
  fs.writeFileSync(path.join(product, 'ACBlackFlag[Options].save'), '');
  enableUpcForGame(exe, tmp);
  assert.match(fs.readFileSync(path.join(gameDir, 'upc_r2.ini'), 'utf8'), /Achievements = 1/);
  const upc = JSON.parse(fs.readFileSync(path.join(product, 'achievements.json'), 'utf8'));
  assert.equal(upc['33'].displayName, 'Yo Ho');
  assert.equal(upc['33'].earned, false);
});

test('ensureGoldbergUpcForGame source steam leaves loader', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'upc-steam-'));
  const exe = path.join(gameDir, 'game.exe');
  fs.writeFileSync(exe, '');
  fs.writeFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-OTHER');
  const cache = fs.mkdtempSync(path.join(tmp, 'upc-cache-'));
  fs.writeFileSync(path.join(cache, 'upc_r2_loader64.dll'), 'MZ-Goldberg UplayEmu Saves');
  const r = ensureGoldbergUpcForGame({ exePath: exe, source: 'steam', cacheDir: cache });
  assert.equal(r.installed, false);
  assert.equal(fs.readFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'utf8'), 'MZ-OTHER');
});

test('ensureGoldbergUpcForGame replaces non-Goldberg loader', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'upc-swap-'));
  const exe = path.join(gameDir, 'game.exe');
  fs.writeFileSync(exe, '');
  fs.writeFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-OTHER');
  const cache = fs.mkdtempSync(path.join(tmp, 'upc-cache2-'));
  fs.writeFileSync(path.join(cache, 'upc_r2_loader64.dll'), 'MZ-Goldberg UplayEmu Saves');
  const r = ensureGoldbergUpcForGame({ exePath: exe, source: 'custom', cacheDir: cache });
  assert.equal(r.installed, true);
  assert.equal(fs.readFileSync(path.join(gameDir, 'upc_r2_loader64.dll.bak'), 'utf8'), 'MZ-OTHER');
  assert.match(
    fs.readFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'utf8'),
    /Goldberg UplayEmu Saves/,
  );
});

test('ensureGoldbergUpcForGame keeps Goldberg loader', () => {
  const gameDir = fs.mkdtempSync(path.join(tmp, 'upc-keep-'));
  const exe = path.join(gameDir, 'game.exe');
  fs.writeFileSync(exe, '');
  fs.writeFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-Goldberg UplayEmu Saves EXISTING');
  const cache = fs.mkdtempSync(path.join(tmp, 'upc-cache3-'));
  fs.writeFileSync(path.join(cache, 'upc_r2_loader64.dll'), 'MZ-Goldberg UplayEmu Saves NEW');
  const r = ensureGoldbergUpcForGame({ exePath: exe, source: 'custom', cacheDir: cache });
  assert.equal(r.installed, false);
  assert.match(
    fs.readFileSync(path.join(gameDir, 'upc_r2_loader64.dll'), 'utf8'),
    /EXISTING/,
  );
});
