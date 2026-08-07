export {
  achievementsDbPath,
  getAchievementsDb,
  upsertTrackedGame,
  upsertAchievementDef,
  recordUnlock,
  listTrackedGames,
  listAchievementsForGame,
  type AchievementSource,
  type TrackedGame,
  type AchievementRow,
} from './store.js';

export {
  GUIDES,
  detectExe,
  detectEosVendor,
  getEmuGuide,
  trackGameFromExe,
  findGameForProcess,
  type EmuGuide,
  type DetectResult,
  type EpicEosVendor,
} from './detect.js';

export {
  importDefsFromGameDir,
  resolveLibraryExePath,
  syncLibraryGame,
} from './schema-import.js';

export {
  resolveEpicNamespace,
  fetchAndImportEpicAchievements,
  writeNemirtingasSetup,
  setStoredEpicNamespace,
  getStoredEpicNamespace,
  toEpicProductSlug,
} from './epic-fetch.js';
