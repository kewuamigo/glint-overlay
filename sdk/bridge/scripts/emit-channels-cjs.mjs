import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import * as channels from '../dist/channels.js';

const out = fileURLToPath(new URL('../dist/channels.cjs', import.meta.url));
const { HostIpc, HostEvents } = channels;

writeFileSync(
  out,
  `"use strict";
exports.HostIpc = ${JSON.stringify(HostIpc, null, 2)};
exports.HostEvents = ${JSON.stringify(HostEvents, null, 2)};
`,
);
