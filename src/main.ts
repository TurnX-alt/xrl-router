import { createApp } from 'vue';
import { createPinia } from 'pinia';
import { createRouter, createWebHistory } from 'vue-router';
import App from './App.vue';
import { routes } from './router.js';
import { initTheme, initSystemThemeListener } from './theme';
import { initI18n } from './i18n';
import { initFm } from './fm/player';

// 应用持久化的主题（在 mount 前设置，避免闪烁）
initTheme();
initSystemThemeListener();

// 初始化国际化
initI18n();

// Claude FM 初始化：连接后端引擎，获取初始状态并监听事件
initFm();

// Material Web Components — 入口必需组件（ConnectionStatus / PluginRegisterDialog 使用）
// 其余组件由各视图按需 import，利用路由懒加载自动 code-split
import '@material/web/button/filled-button.js';
import '@material/web/button/text-button.js';
import '@material/web/icon/icon.js';
import '@material/web/dialog/dialog.js';

const app = createApp(App);
const pinia = createPinia();
const router = createRouter({
  history: createWebHistory(),
  routes,
});

// Global error handler for Vue components
app.config.errorHandler = (err, instance, info) => {
  console.error('[Vue Error]', err);
  console.error('[Component]', instance?.$options?.name || 'Anonymous');
  console.error('[Info]', info);
};

// Global error handler for unhandled promise rejections
window.addEventListener('unhandledrejection', (event) => {
  console.error('[Unhandled Rejection]', event.reason);
});

app.use(pinia);
app.use(router);
app.mount('#app');