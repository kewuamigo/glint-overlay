export {
  achievementsDbPath,
  getAchievementsDb,
  upsertTrackedGame,
  upsertAchievementDef,
  recordUnlock,
  listTrackedGames,
  listAchievementsForGame,
  setPrepareStatus,
  getPrepareStatus,
  listPrepareStatuses,
  setLibraryIdForAppId,
  getLibraryIdForAppId,
  type AchievementSource,
  type TrackedGame,
  type AchievementRow,
  type PrepareStatus,
  type PrepareState,
} from './store.js';

export {
  GUIDES,
  detectExe,
  detectEosVendor,
  detectScanDirs,
  gameRootFromExe,
  findGameExeInFolder,
  getEmuGuide,
  resolveSteamAppId,
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

export {
  fetchAndImportSteamAchievements,
  writeGseAchievementsJson,
} from './steam-fetch.js';

export {
  GSE_ASSET_URL,
  GSE_ASSET_NAME,
  gseAssetUrl,
  resolveLatestGseTag,
  ensureGseCache,
  ensureGseForGame,
} from './gse-cache.js';

export {
  prepareGame,
  prepareBlocksLaunch,
  type PrepareGameInput,
} from './prepare.js';

export {
  mapUpcInId,
  readSchemaNames,
  mergeGseUnlock,
  gseUnlockPath,
  bridgeUpcUnlock,
  enableUpcForGame,
  ensureGoldbergUpcForGame,
  isGoldbergUpc,
  defaultUpcCacheDir,
} from './upc-bridge.js';
