const runtime = globalThis.__goSharedJsxRuntime;
if (!runtime) {
  throw new Error('Shared jsx-runtime not initialized — load overlay UI before plugins');
}
export const { Fragment, jsx, jsxs } = runtime;
