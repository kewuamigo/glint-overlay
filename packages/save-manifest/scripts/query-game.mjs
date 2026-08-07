import { DatabaseSync } from 'node:sqlite';
import path from 'node:path';
import os from 'node:os';

const title = process.argv[2] ?? 'Alan Wake 2';
const dbPath = path.join(os.homedir(), 'AppData/Roaming/Glint/apps/save-manager/data/manifest.db');
const db = new DatabaseSync(dbPath, { readOnly: true });
const game = db.prepare('SELECT title, steam_id, notes FROM games WHERE title = ?').get(title);
console.log('game:', game);
const files = db.prepare('SELECT path_template, tags FROM game_files WHERE game_title = ?').all(title);
console.log('files:', files.length);
for (const f of files) console.log(' ', f.path_template, JSON.parse(f.tags));
db.close();
