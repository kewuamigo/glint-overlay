import { copyFileSync, readdirSync } from 'node:fs';
import path from 'node:path';

const testDir = 'test';
for (const name of readdirSync(testDir)) {
  if (!name.endsWith('.test.ts')) continue;
  copyFileSync(path.join(testDir, name), path.join(testDir, name.replace(/\.ts$/, '.js')));
}
