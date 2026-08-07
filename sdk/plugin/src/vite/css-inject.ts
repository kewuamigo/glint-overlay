/**
 * Custom inject body for `vite-plugin-css-injected-by-js`.
 * Routes CSS into the overlay host's plugin Shadow DOM when available.
 *
 * @param pluginId When set, baked into the bundle so parallel plugin loads stay isolated.
 */
export function isolatedCssInjectCode(cssCode: string, pluginId?: string): string {
  const encoded = JSON.stringify(cssCode);
  const idExpr = pluginId ? JSON.stringify(pluginId) : 'window.__goActivePluginId';
  return `try{var pid=${idExpr};if(typeof window!=="undefined"&&typeof window.__goQueuePluginCss==="function"&&pid){window.__goQueuePluginCss(pid,${encoded});}else if(typeof document!=="undefined"){var el=document.createElement("style");el.appendChild(document.createTextNode(${encoded}));document.head.appendChild(el);}}catch(e){console.error("plugin-css-inject",e);}`;
}
