// 主题切换：light / dark / system，持久化到 localStorage，默认跟随系统。
// 通过 <html data-theme="light|dark"> 触发 index.html 里的 token 切换，
// 同时同步 Tauri 原生窗口主题，使标题栏等系统 UI 跟随。
//
// 重要：Tauri 中 window.setTheme('light'/'dark') 会强制窗口主题，而 WebView
// 的 prefers-color-scheme media query 跟随窗口主题。因此「跟随系统」模式下
// 必须 setTheme(null) 取消强制，media query 才能反映真实系统主题——
// 否则曾选过深色后，media query 永远返回深色，跟随系统失效。

import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'light' | 'dark' | 'system';

const STORAGE_KEY = 'theme';

function systemTheme(): 'light' | 'dark' {
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function getTheme(): Theme {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === 'light' || saved === 'dark') return saved;
  return 'system';
}

// 同步 Tauri 原生窗口主题（标题栏等系统级 UI）。
// t 为 null = 取消强制，窗口恢复跟随系统（WebView media query 同步恢复真实值）。
// 非 Tauri 环境（纯浏览器调试）调用会抛错，静默忽略。
async function applyWindowTheme(t: 'light' | 'dark' | null) {
  try {
    await getCurrentWindow().setTheme(t);
  } catch {
    // 非 Tauri 环境无原生窗口，忽略
  }
}

export function setTheme(t: Theme) {
  localStorage.setItem(STORAGE_KEY, t);
  if (t === 'system') {
    document.documentElement.setAttribute('data-theme', systemTheme());
    // 取消窗口强制 → WebView prefers-color-scheme 恢复跟随系统
    void applyWindowTheme(null);
  } else {
    document.documentElement.setAttribute('data-theme', t);
    void applyWindowTheme(t);
  }
}

export function initTheme() {
  const t = getTheme();
  if (t === 'system') {
    document.documentElement.setAttribute('data-theme', systemTheme());
    void applyWindowTheme(null);
  } else {
    document.documentElement.setAttribute('data-theme', t);
    void applyWindowTheme(t);
  }
}

// 监听系统主题变化，当用户选择「跟随系统」时自动响应。
// 窗口已取消强制（跟随系统），media query 会随系统变化自动触发 change。
export function initSystemThemeListener() {
  const mq = window.matchMedia('(prefers-color-scheme: dark)');
  mq.addEventListener('change', (e) => {
    if (getTheme() === 'system') {
      document.documentElement.setAttribute('data-theme', e.matches ? 'dark' : 'light');
    }
  });
}
