// 极简 i18n 实现：zh-CN / en 双语，响应式，持久化到 localStorage，
// 切换语言时同步后端（托盘菜单等原生 UI 文本）。

import { reactive } from 'vue';
import zhCN from './zh-CN';
import en from './en';
import { settingsApi } from '../api';

export type Locale = 'zh-CN' | 'en';

const STORAGE_KEY = 'locale';

// 翻译字典
const dictionaries: Record<Locale, Record<string, string>> = {
  'zh-CN': zhCN,
  'en': en,
};

// 当前语言（响应式）
export const i18n = reactive({
  locale: 'zh-CN' as Locale,
});

// 初始化：从 localStorage 读取偏好
export function initI18n() {
  const saved = localStorage.getItem(STORAGE_KEY) as Locale | null;
  const locale = saved && ['zh-CN', 'en'].includes(saved) ? saved : 'zh-CN';
  i18n.locale = locale;
}

// 同步后端（Tauri）：托盘菜单等原生 UI 文本随语言切换。
// 非 Tauri 环境（纯浏览器调试）invoke 抛错，静默忽略。
async function syncToBackend(locale: Locale) {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('set_locale', { locale });
  } catch {
    // 非 Tauri 环境无原生菜单，忽略
  }
}

// 同步 UI 设置到后端（LAN install 页面可读取）
async function syncLocaleToBackend(locale: Locale) {
  try {
    await settingsApi.update({ locale });
  } catch {
    // API 不可用，忽略
  }
}

// 切换语言
export function setLocale(locale: Locale) {
  i18n.locale = locale;
  localStorage.setItem(STORAGE_KEY, locale);
  void syncToBackend(locale);
  void syncLocaleToBackend(locale);
}

// 翻译函数
export function t(key: string, params?: Record<string, string | number>): string {
  const dict = dictionaries[i18n.locale];
  let text = dict[key] ?? key;

  // 简单参数替换：{name} -> value
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      text = text.replace(new RegExp(`\\{${k}\\}`, 'g'), String(v));
    }
  }

  return text;
}
