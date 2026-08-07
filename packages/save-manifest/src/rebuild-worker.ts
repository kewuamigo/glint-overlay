// Worker-thread entry: parsing the ~17MB Ludusavi YAML and rebuilding the
// SQLite database takes tens of seconds and MUST NOT run on the Electron main
// thread (it freezes all overlay input while it runs).
import { parentPort, workerData } from 'node:worker_threads';
import { buildDbFromYaml } from './build-db.js';

const { yamlPath, dbPath } = workerData as { yamlPath: string; dbPath: string };

try {
  buildDbFromYaml(yamlPath, dbPath);
  parentPort?.postMessage({ ok: true });
} catch (err) {
  parentPort?.postMessage({
    ok: false,
    error: err instanceof Error ? err.message : String(err),
  });
}
