"use strict";
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("__goLauncher", {
  invoke: (method, args = []) =>
    ipcRenderer.invoke("launcher:invoke", { method, args }),
  onSyncStatus: (cb) => {
    if (typeof cb !== "function") return () => {};
    const handler = (_event, payload) => cb(payload);
    ipcRenderer.on("cloud.sync.status", handler);
    return () => ipcRenderer.removeListener("cloud.sync.status", handler);
  },
  onOtaStatus: (cb) => {
    if (typeof cb !== "function") return () => {};
    const handler = (_event, payload) => cb(payload);
    ipcRenderer.on("ota.status", handler);
    return () => ipcRenderer.removeListener("ota.status", handler);
  },
});
