import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';

export default defineConfig({
  plugins: [react()],
  // Relative asset URLs so the dist also loads from file:// in the CEF host.
  base: './',
  optimizeDeps: {
    include: ['motion', '@radix-ui/react-tooltip'],
  },
  ssr: {
    noExternal: ['motion'],
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: path.resolve(__dirname, 'index.html'),
    },
  },
});
