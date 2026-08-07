import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { buildDbFromYaml } from '../dist/build-db.js';

function defaultCacheDir() {
  const appData = process.env.APPDATA ?? path.join(os.homedir(), 'AppData', 'Roaming');
  return path.join(appData, 'Glint', 'apps', 'save-manager', 'data');
}

function resolveYamlPath() {
  const cacheDir = defaultCacheDir();
  const cached = path.join(cacheDir, 'manifest.yaml');
  if (fs.existsSync(cached)) {
    return cached;
  }

  const repoFixture = path.resolve(
    path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')),
    '../../../data/save-manifest/fixture.yaml',
  );
  if (fs.existsSync(repoFixture)) {
    return repoFixture;
  }

  throw new Error('No Ludusavi YAML found to build manifest.db');
}

const yamlPath = process.argv[2] ?? resolveYamlPath();
const dbPath = process.argv[3] ?? path.join(defaultCacheDir(), 'manifest.db');
const started = Date.now();
buildDbFromYaml(yamlPath, dbPath);
console.log(`Built ${dbPath} from ${yamlPath} in ${Date.now() - started}ms`);
