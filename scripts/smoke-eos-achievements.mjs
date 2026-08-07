/**
 * Smoke-test achievement ingest without the game.
 * Usage (from repo root):
 *   node scripts/smoke-eos-achievements.mjs
 */
import fs from 'node:fs';
import path from 'node:path';
import {
  findGameForProcess,
  listAchievementsForGame,
  listTrackedGames,
  recordUnlock,
} from '../packages/achievements-core/dist/index.js';

const exe = 'AlanWake2.exe';
const candidates = listTrackedGames().filter(
  (g) => g.process_name && g.process_name.toLowerCase() === exe.toLowerCase(),
);
const game =
  candidates.find((g) => listAchievementsForGame(g.id).length > 0) ??
  findGameForProcess('', exe);
if (!game) {
  console.error('FAIL: no tracked game for', exe, '— sync achievements in the launcher first');
  process.exit(1);
}

const defs = listAchievementsForGame(game.id);
const sample = defs[0];
if (!sample) {
  console.error('FAIL: no achievement defs for', game.id, '— open Achievements tab once to sync');
  process.exit(1);
}

const before = listAchievementsForGame(game.id).filter((r) => r.unlocked).length;
const isNew = recordUnlock({
  gameId: game.id,
  achievementId: sample.achievement_id,
  title: sample.title,
  description: sample.description,
  iconUnlocked: sample.icon_unlocked,
});
const after = listAchievementsForGame(game.id).filter((r) => r.unlocked).length;

const hooksDir = path.join(
  process.env.APPDATA ?? '',
  'Glint',
  'apps',
  'achievements',
);
fs.mkdirSync(hooksDir, { recursive: true });
const hooksPath = path.join(hooksDir, 'eos-hooks.jsonl');
fs.appendFileSync(
  hooksPath,
  `${JSON.stringify({
    kind: 'smoke',
    pid: process.pid,
    exe,
    ids: [sample.achievement_id],
    ts: Date.now(),
  })}\n`,
);

console.log(
  JSON.stringify(
    {
      ok: true,
      gameId: game.id,
      achievementId: sample.achievement_id,
      title: sample.title,
      isNew,
      unlockedBefore: before,
      unlockedAfter: after,
      hooksPath,
      next: 'With overlay running, a real unlock needs kind=unlock lines from the metrics DLL. After relaunch, check eos-hooks.jsonl for kind=installed.',
    },
    null,
    2,
  ),
);
