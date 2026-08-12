import { defineConfig, type Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import { rmSync } from 'node:fs';
import { resolve } from 'node:path';

/** macOS 自带苹方（运行时 @font-face 的 local() 命中系统字体），
    打包时剔除 webfont 以减小体积；其他平台照常携带。
    可用 VITE_STRIP_PINGFANG=1 在任意平台强制剔除（也便于验证） */
function stripPingFangOnDarwin(): Plugin {
  let outDir = '';
  return {
    name: 'strip-pingfang-on-darwin',
    apply: 'build',
    configResolved(cfg) {
      outDir = resolve(cfg.root, cfg.build.outDir);
    },
    closeBundle() {
      const strip = process.env.VITE_STRIP_PINGFANG === '1' || process.platform === 'darwin';
      if (!strip) return;
      rmSync(resolve(outDir, 'fonts'), { recursive: true, force: true });
      console.log('[strip-pingfang-on-darwin] removed fonts/ from', outDir);
    },
  };
}

export default defineConfig({
  plugins: [
    vue({
      // MD3 Web Components 是原生 custom element（main.ts 副作用 import 注册），
      // 不是 Vue 组件：告知编译器避免 "Failed to resolve component: md-*" 警告
      template: {
        compilerOptions: {
          isCustomElement: (tag: string) => tag.startsWith('md-'),
        },
      },
    }),
    stripPingFangOnDarwin(),
  ],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:19068',
      '/v1': 'http://127.0.0.1:19068',
      '/health': 'http://127.0.0.1:19068',
      '/fm': 'http://127.0.0.1:19068',
      '/install': 'http://127.0.0.1:19068',
      '/ws': {
        target: 'ws://127.0.0.1:19068',
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
  },
});