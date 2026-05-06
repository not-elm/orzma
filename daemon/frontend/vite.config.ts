import babel from '@rolldown/plugin-babel';
import tailwindcss from '@tailwindcss/vite';
import react, { reactCompilerPreset } from '@vitejs/plugin-react';
import { defineConfig } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss(), viteSingleFile()],
  build: {
    outDir: '../http_server/src/handlers',
    emptyOutDir: false,
  },
  server: {
    proxy: {
      '/sessions': 'http://127.0.0.1:3200',
      '/activities': { target: 'http://127.0.0.1:3200', ws: true },
      '/health': 'http://127.0.0.1:3200',
    },
  },
});
