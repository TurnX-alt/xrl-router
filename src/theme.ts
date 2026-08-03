// 主题切换：light / dark，持久化到 localStorage，默认跟随系统。
// 通过 <html data-theme="light|dark"> 触发 index.html 里的 token 切换，
// 同时同步 Tauri 原生窗口主题，使标题栏等系统 UI 跟随。

import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'theme';

function systemTheme(): Theme {
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function getTheme(): Theme {
  const saved = localStorage.getItem(STORAGE_KEY);
  return saved === 'light' || saved === 'dark' ? saved : systemTheme();
}

// 同步 Tauri 原生窗口主题（标题栏等系统级 UI）。
// 非 Tauri 环境（纯浏览器调试）调用会抛错，静默忽略。
async function applyWindowTheme(t: Theme) {
  try {
    await getCurrentWindow().setTheme(t);
  } catch {
    // 非 Tauri 环境无原生窗口，忽略
  }
}

export function setTheme(t: Theme) {
  localStorage.setItem(STORAGE_KEY, t);
  document.documentElement.setAttribute('data-theme', t);
  void applyWindowTheme(t);
}

export function initTheme() {
  const t = getTheme();
  document.documentElement.setAttribute('data-theme', t);
  void applyWindowTheme(t);
}
