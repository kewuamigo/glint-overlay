import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolvePlaceholders } from '../dist/placeholder-resolver.js';

test('resolves winAppData and storeGameDir', () => {
  const ctx = {
    home: 'C:\\Users\\dev',
    winLocalAppData: 'C:\\Users\\dev\\AppData\\Local',
    winAppData: 'C:\\Users\\dev\\AppData\\Roaming',
    winDocuments: 'C:\\Users\\dev\\Documents',
    storeGameDir: 'D:\\Steam\\steamapps\\common\\Hades',
    storeUserDir: 'D:\\Steam\\steamapps\\common\\Hades',
    storeUserId: 'C:\\Program Files (x86)\\Steam\\userdata\\123\\1145360',
    root: 'D:\\Steam',
  };
  const out = resolvePlaceholders('<winAppData>/Hades', ctx);
  assert.equal(out, 'C:\\Users\\dev\\AppData\\Roaming\\Hades');
});
