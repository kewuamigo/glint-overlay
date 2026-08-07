import { copyFileSync } from 'node:fs';

copyFileSync('test/placeholder-resolver.test.ts', 'test/placeholder-resolver.test.js');
