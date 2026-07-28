import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:19068',
      '/v1': 'http://localhost:19068',
      '/health': 'http://localhost:19068',
      '/ws': {
        target: 'ws://localhost:19068',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
  },
});