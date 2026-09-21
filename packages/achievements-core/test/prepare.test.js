import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'go-ach-prepare-'));
process.env.APPDATA = tmp;

const {
  achievementsDbPath,
  setPrepareStatus,
  getPrepareStatus,
  setLibraryIdForAppId,
  getLibraryIdForAppId,
  importDefsFromGameDir,
  listAchievementsForGame,
  fetchAndImportSteamAchievements,
  prepareGame,
  prepareBlocksLaunch,
} = await import('../dist/index.js');

test('tests write to temp APPDATA, not the user Glint DB', () => {
  assert.ok(achievementsDbPath().startsWith(tmp));
});

test('set/get pending, ready, failed (with error), skipped', () => {
  setPrepareStatus('lib-pending', 'pending');
  const pending = getPrepareStatus('lib-pending');
  assert.equal(pending?.status, 'pending');
  assert.equal(typeof pending?.updatedAt, 'number');

  setPrepareStatus('lib-ready', 'ready');
  assert.equal(getPrepareStatus('lib-ready')?.status, 'ready');

  setPrepareStatus('lib-failed', 'failed', { error: 'no appid' });
  const failed = getPrepareStatus('lib-failed');
  assert.equal(failed?.status, 'failed');
  assert.equal(failed?.error, 'no appid');

  setPrepareStatus('lib-skipped', 'skipped');
  assert.equal(getPrepareStatus('lib-skipped')?.status, 'skipped');
});

test('missing prepare key returns null', () => {
  assert.equal(getPrepareStatus('missing-lib'), null);
});

test('reverse map appid to library id', () => {
  setLibraryIdForAppId('3321460', 'lib-crimson');
  assert.equal(getLibraryIdForAppId('3321460'), 'lib-crimson');
});

test('missing appid map returns null', () => {
  assert.equal(getLibraryIdForAppId('999999'), null);
});

test('ready with steamAppId writes library_by_appid', () => {
  setPrepareStatus('lib-mapped', 'ready', { steamAppId: '480' });
  const state = getPrepareStatus('lib-mapped');
  assert.equal(state?.status, 'ready');
  assert.equal(state?.steamAppId, '480');
  assert.equal(getLibraryIdForAppId('480'), 'lib-mapped');
});

test('importDefsFromGameDir reads GSE object map from steam_settings/achievements.json', () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-ach-gse-schema-'));
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  fs.writeFileSync(
    path.join(gameDir, 'steam_settings', 'achievements.json'),
    JSON.stringify({
      ACH_WIN: { name: 'Winner', description: 'Win a match' },
      ACH_LOSE: { name: 'Learner', description: 'Lose once' },
    }),
  );
  const gameId = 'custom:test-gse';
  const count = importDefsFromGameDir(gameId, gameDir);
  assert.equal(count, 2);
  const defs = listAchievementsForGame(gameId);
  assert.equal(defs.length, 2);
  const win = defs.find((d) => d.achievement_id === 'ACH_WIN');
  assert.equal(win?.title, 'Winner');
  assert.equal(win?.description, 'Win a match');
});

const steamSchema = {
  game: {
    availableGameStats: {
      achievements: [
        {
          name: 'ACH_WIN',
          displayName: 'Winner',
          description: 'Win a match',
          icon: 'https://example.com/win.png',
          icongray: 'https://example.com/win_gray.png',
          hidden: 0,
        },
        {
          name: 'ACH_LOSE',
          displayName: 'Learner',
          description: 'Lose once',
          icon: 'https://example.com/lose.png',
          icongray: 'https://example.com/lose_gray.png',
          hidden: 1,
        },
      ],
    },
  },
};

test('fetchAndImportSteamAchievements upserts defs under library gameId', async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = async (input) => {
    const url = String(input);
    assert.ok(url.includes('ISteamUserStats/GetSchemaForGame/v2'));
    assert.ok(url.includes('key=test-key'));
    assert.ok(url.includes('appid=3321460'));
    return {
      ok: true,
      status: 200,
      json: async () => steamSchema,
    };
  };
  try {
    const gameId = 'custom:crimson';
    const count = await fetchAndImportSteamAchievements(
      gameId,
      '3321460',
      'test-key',
    );
    assert.equal(count, 2);
    const defs = listAchievementsForGame(gameId);
    assert.equal(defs.length, 2);
    const win = defs.find((d) => d.achievement_id === 'ACH_WIN');
    assert.equal(win?.title, 'Winner');
    assert.equal(win?.description, 'Win a match');
  } finally {
    globalThis.fetch = orig;
  }
});

test('fetchAndImportSteamAchievements throws when API key is missing', async () => {
  await assert.rejects(
    () => fetchAndImportSteamAchievements('custom:x', '480', ''),
    /api key/i,
  );
});

test('fetchAndImportSteamAchievements throws when AppID is missing', async () => {
  await assert.rejects(
    () => fetchAndImportSteamAchievements('custom:x', '', 'test-key'),
    /appid/i,
  );
});

test('fetchAndImportSteamAchievements throws on HTTP 403', async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = async () => ({
    ok: false,
    status: 403,
    json: async () => ({}),
  });
  try {
    await assert.rejects(
      () => fetchAndImportSteamAchievements('custom:x', '480', 'test-key'),
      /403/,
    );
  } finally {
    globalThis.fetch = orig;
  }
});

test('fetchAndImportSteamAchievements writes GSE array achievements.json when steam_settings exists', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-ach-gse-write-'));
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  const orig = globalThis.fetch;
  globalThis.fetch = async () => ({
    ok: true,
    status: 200,
    json: async () => steamSchema,
  });
  try {
    await fetchAndImportSteamAchievements(
      'custom:gse-write',
      '3321460',
      'test-key',
      gameDir,
    );
    const written = JSON.parse(
      fs.readFileSync(
        path.join(gameDir, 'steam_settings', 'achievements.json'),
        'utf8',
      ),
    );
    assert.ok(Array.isArray(written));
    assert.deepEqual(
      written.map((row) => row.name).sort(),
      ['ACH_LOSE', 'ACH_WIN'],
    );
    const win = written.find((row) => row.name === 'ACH_WIN');
    assert.equal(win.displayName, 'Winner');
    assert.equal(win.description, 'Win a match');
    assert.equal(win.icon, undefined);
    assert.equal(win.icongray, undefined);
    assert.equal(win.hidden, 0);
    const lose = written.find((row) => row.name === 'ACH_LOSE');
    assert.equal(lose.displayName, 'Learner');
    assert.equal(lose.hidden, 1);
  } finally {
    globalThis.fetch = orig;
  }
});

test('fetchAndImportSteamAchievements does not create steam_settings when missing', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-ach-gse-nowrite-'));
  const orig = globalThis.fetch;
  globalThis.fetch = async () => ({
    ok: true,
    status: 200,
    json: async () => steamSchema,
  });
  try {
    await fetchAndImportSteamAchievements(
      'custom:gse-nowrite',
      '3321460',
      'test-key',
      gameDir,
    );
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_settings')), false);
  } finally {
    globalThis.fetch = orig;
  }
});

function writeFile(filePath, contents) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, contents);
}

function fixtureCache() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-cache-fx-'));
  writeFile(path.join(dir, 'steam_api64.dll'), 'MZ-FROM-CACHE');
  writeFile(path.join(dir, 'steam_api.dll'), 'MZ-FROM-CACHE-32');
  return dir;
}

function stubSteamSchemaFetch() {
  const orig = globalThis.fetch;
  globalThis.fetch = async () => ({
    ok: true,
    status: 200,
    json: async () => steamSchema,
  });
  return () => {
    globalThis.fetch = orig;
  };
}

test('prepareGame with no steam/eos dll is skipped', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-skip-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  const id = 'custom:plain-skip';
  const result = await prepareGame({
    id,
    name: 'Plain',
    exe,
    install_path: gameDir,
    source: 'custom',
  });
  assert.equal(result.status, 'skipped');
  assert.equal(getPrepareStatus(id)?.status, 'skipped');
});

test('prepareGame source steam does not rename dlls', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-steam-src-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  writeFile(path.join(gameDir, 'steam_appid.txt'), '480\n');
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame({
      id: 'steam:480-prep',
      name: 'Half-Life',
      exe,
      install_path: gameDir,
      source: 'steam',
      steamApiKey: 'test-key',
    });
    assert.equal(result.status, 'ready');
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'),
      'MZ-ORIGINAL',
    );
    assert.equal(fs.existsSync(path.join(gameDir, 'steam_api64.dll.bak')), false);
  } finally {
    restore();
  }
});

test('prepareGame custom plain steam_api + key + appid is ready with defs and reverse map', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-custom-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  writeFile(path.join(gameDir, 'steam_appid.txt'), '3321460\n');
  const restore = stubSteamSchemaFetch();
  try {
    const id = 'custom:crimson-prep';
    const result = await prepareGame(
      {
        id,
        name: 'Crimson Desert',
        exe,
        install_path: gameDir,
        source: 'custom',
        steamApiKey: 'test-key',
      },
      { cacheDir: fixtureCache() },
    );
    assert.equal(result.status, 'ready');
    assert.equal(result.steamAppId, '3321460');
    assert.equal(getLibraryIdForAppId('3321460'), id);
    const defs = listAchievementsForGame(id);
    assert.equal(defs.length, 2);
    assert.ok(defs.find((d) => d.achievement_id === 'ACH_WIN'));
  } finally {
    restore();
  }
});

test('prepareGame watchable GSE writes array steam_settings/achievements.json', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-watchable-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-gbe_fork');
  fs.mkdirSync(path.join(gameDir, 'steam_settings'));
  writeFile(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), '3321460\n');
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame({
      id: 'custom:watchable-gse',
      name: 'Crimson Desert',
      exe,
      install_path: gameDir,
      source: 'custom',
      steamApiKey: 'test-key',
    });
    assert.equal(result.status, 'ready');
    const written = JSON.parse(
      fs.readFileSync(
        path.join(gameDir, 'steam_settings', 'achievements.json'),
        'utf8',
      ),
    );
    assert.ok(Array.isArray(written));
    assert.equal(written[0].name, 'ACH_WIN');
    assert.equal(written[0].displayName, 'Winner');
    assert.equal(written[0].icon, undefined);
    assert.equal(listAchievementsForGame('custom:watchable-gse').length, 2);
  } finally {
    restore();
  }
});

test('prepareGame missing key on steam custom is failed', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-nokey-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  writeFile(path.join(gameDir, 'steam_appid.txt'), '480\n');
  const result = await prepareGame(
    {
      id: 'custom:nokey',
      name: 'No Key',
      exe,
      install_path: gameDir,
      source: 'custom',
    },
    { cacheDir: fixtureCache() },
  );
  assert.equal(result.status, 'failed');
  assert.match(result.error ?? '', /api key/i);
});

test('prepareGame missing AppID on steam custom is failed', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-noappid-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  const id = 'custom:noappid';
  const result = await prepareGame({
    id,
    name: 'No AppID',
    exe,
    install_path: gameDir,
    source: 'custom',
    steamApiKey: 'test-key',
  });
  assert.equal(result.status, 'failed');
  assert.match(result.error ?? '', /appid/i);
  assert.equal(getPrepareStatus(id)?.status, 'failed');
});

test('prepareGame forceGse on uplay-only folder installs GSE', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-force-uplay-'));
  const exe = path.join(gameDir, 'ACShadows.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-UPC');
  writeFile(path.join(gameDir, 'upc_r2.ini'), '[Settings]\nAchievements = 0\n');
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame(
      {
        id: 'custom:ac-shadows',
        name: "Assassin's Creed Shadows",
        exe,
        install_path: gameDir,
        source: 'custom',
        steamApiKey: 'test-key',
        steamAppId: '3159330',
        forceGse: true,
      },
      { cacheDir: fixtureCache() },
    );
    assert.equal(result.status, 'ready');
    assert.equal(result.steamAppId, '3159330');
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'),
      'MZ-FROM-CACHE',
    );
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3159330',
    );
  } finally {
    restore();
  }
});

test('prepareGame forceGse without AppID fails instead of skipping', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-force-noappid-'));
  const exe = path.join(gameDir, 'ACShadows.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'upc_r2_loader64.dll'), 'MZ-UPC');
  const result = await prepareGame({
    id: 'custom:ac-shadows-noappid',
    name: "Assassin's Creed Shadows",
    exe,
    install_path: gameDir,
    source: 'custom',
    steamApiKey: 'test-key',
    forceGse: true,
  });
  assert.equal(result.status, 'failed');
  assert.match(result.error ?? '', /appid/i);
});

test('prepareGame forceGse on source steam installs GSE', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-force-gse-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame(
      {
        id: 'steam:3159330',
        name: "Assassin's Creed Shadows",
        exe,
        install_path: gameDir,
        source: 'steam',
        steamApiKey: 'test-key',
        forceGse: true,
      },
      { cacheDir: fixtureCache() },
    );
    assert.equal(result.status, 'ready');
    assert.equal(result.steamAppId, '3159330');
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'),
      'MZ-FROM-CACHE',
    );
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll.bak'), 'utf8'),
      'MZ-ORIGINAL',
    );
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3159330',
    );
  } finally {
    restore();
  }
});

test('prepareGame steam library id supplies AppID when disk files missing', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-steam-id-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  const id = 'steam:3751950';
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame({
      id,
      name: "Assassin's Creed Black Flag Resynced",
      exe,
      install_path: gameDir,
      source: 'steam',
      steamApiKey: 'test-key',
    });
    assert.equal(result.status, 'ready');
    assert.equal(result.steamAppId, '3751950');
    assert.equal(getLibraryIdForAppId('3751950'), id);
  } finally {
    restore();
  }
});

test('prepareGame custom uses caller steamAppId when disk files missing', async () => {
  const gameDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-prep-ui-appid-'));
  const exe = path.join(gameDir, 'game.exe');
  writeFile(exe, '');
  writeFile(path.join(gameDir, 'steam_api64.dll'), 'MZ-ORIGINAL');
  const id = 'custom:black-flag-resynced';
  const restore = stubSteamSchemaFetch();
  try {
    const result = await prepareGame(
      {
        id,
        name: "Assassin's Creed Black Flag Resynced",
        exe,
        install_path: gameDir,
        source: 'custom',
        steamApiKey: 'test-key',
        steamAppId: '3751950',
      },
      { cacheDir: fixtureCache() },
    );
    assert.equal(result.status, 'ready');
    assert.equal(result.steamAppId, '3751950');
    assert.equal(getLibraryIdForAppId('3751950'), id);
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_api64.dll'), 'utf8'),
      'MZ-FROM-CACHE',
    );
    assert.ok(fs.existsSync(path.join(gameDir, 'steam_api64.dll.bak')));
    assert.equal(
      fs.readFileSync(path.join(gameDir, 'steam_settings', 'steam_appid.txt'), 'utf8').trim(),
      '3751950',
    );
  } finally {
    restore();
  }
});

test('prepareBlocksLaunch is true for pending, failed, and missing', () => {
  assert.equal(prepareBlocksLaunch({ status: 'pending', updatedAt: 1 }), true);
  assert.equal(
    prepareBlocksLaunch({ status: 'failed', updatedAt: 1, error: 'no appid' }),
    true,
  );
  assert.equal(prepareBlocksLaunch(null), true);
});

test('prepareBlocksLaunch is false for ready and skipped', () => {
  assert.equal(prepareBlocksLaunch({ status: 'ready', updatedAt: 1 }), false);
  assert.equal(prepareBlocksLaunch({ status: 'skipped', updatedAt: 1 }), false);
});
