// 主题切换：light / dark，持久化到 localStorage，默认跟随系统。
// 通过 <html data-theme="light|dark"> 触发 index.html 里的 token 切换。

export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'theme';

function systemTheme(): Theme {
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function getTheme(): Theme {
  const saved = localStorage.getItem(STORAGE_KEY);
  return saved === 'light' || saved === 'dark' ? saved : systemTheme();
}

export function setTheme(t: Theme) {
  localStorage.setItem(STORAGE_KEY, t);
  document.documentElement.setAttribute('data-theme', t);
}

export function initTheme() {
  document.documentElement.setAttribute('data-theme', getTheme());
}
