import path from 'node:path';
import { app, BrowserWindow, Menu } from 'electron';
import { fileURLToPath } from 'node:url';
import { migrateAppDataFromGameOverlay } from './appdata-migrate.js';
import { registerLauncherIpc, setLauncherWindow } from './ipc-handlers.js';
import { scheduleOtaStartupCheck } from './ota.js';
import { APP_DATA_DIR_NAME, PRODUCT_NAME } from './product.js';
import { getLauncherUiUrl, startLauncherUiServer } from './ui-server.js';

migrateAppDataFromGameOverlay();

const launcherUserData = path.join(
  process.env.APPDATA ?? '',
  APP_DATA_DIR_NAME,
  'launcher',
);
app.setPath('userData', launcherUserData);

async function createWindow(): Promise<BrowserWindow> {
  const preload = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    '../preload.cjs',
  );

  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    minWidth: 960,
    minHeight: 640,
    title: PRODUCT_NAME,
    backgroundColor: '#0a0c10',
    frame: false,
    show: false,
    webPreferences: {
      preload,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  setLauncherWindow(win);
  win.on('closed', () => setLauncherWindow(null));

  win.once('ready-to-show', () => {
    win.show();
  });

  const uiUrl = getLauncherUiUrl();
  await win.loadURL(uiUrl);
  return win;
}

async function main(): Promise<void> {
  await app.whenReady();
  Menu.setApplicationMenu(null);
  registerLauncherIpc();

  if (!process.env.GLINT_LAUNCHER_UI_URL) {
    const preferred = Number(process.env.GLINT_LAUNCHER_UI_PORT ?? 5174);
    await startLauncherUiServer(preferred);
  }

  await createWindow();
  scheduleOtaStartupCheck();

  app.on('window-all-closed', () => {
    app.quit();
  });
}

main().catch((err) => {
  console.error('launcher fatal:', err);
  app.quit();
  process.exit(1);
});
