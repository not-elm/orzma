import babel from '@rolldown/plugin-babel';
import tailwindcss from '@tailwindcss/vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { viteSingleFile } from 'vite-plugin-singlefile';
import { defineConfig } from 'vitest/config';

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss(), viteSingleFile()],
  build: {
    outDir: '../http_server/src/handlers',
    emptyOutDir: false,
  },
  server: {
    proxy: {
      '/activities': { target: 'http://127.0.0.1:3200', ws: true },
      '/configs': 'http://127.0.0.1:3200',
      '/health': 'http://127.0.0.1:3200',
      '/panes': 'http://127.0.0.1:3200',
      '/sessions': 'http://127.0.0.1:3200',
      '/windows': { target: 'http://127.0.0.1:3200', ws: true },
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
  },
});
