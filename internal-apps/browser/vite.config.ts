import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
  build: {
    lib: {
      entry: 'src/index.tsx',
      formats: ['es'],
      fileName: () => 'index.js',
    },
    rollupOptions: {
      external: ['react', 'react/jsx-runtime'],
      output: {
        paths: {
          react: 'glint-plugin://_shared/react.js',
          'react/jsx-runtime': 'glint-plugin://_shared/react-jsx-runtime.js',
        },
      },
    },
  },
});
