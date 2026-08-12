// Claude FM — 前端播放器（极简版）。
//
// 所有播放逻辑（歌单管理、音源解析、解码、预加载、切歌）在 Rust 后端完成。
// 后端以 rodio 直接输出到系统音频设备，同时接入系统媒体控制（souvlaki：
// macOS Now Playing / Windows SMTC / Linux MPRIS）。
//
// 前端退化为纯展示 + 控制层：
// - 通过 Tauri command 调用 `fm_toggle` / `fm_play` / `fm_pause`
// - 通过 Tauri event 监听 `fm-meta`（切歌）和 `fm-ready`（引擎就绪）
// - 通过 `fm-state-changed` 同步播放/暂停状态到托盘菜单
//
// 生命周期与应用进程绑定，而非任何视图组件：
// - 路由切换不销毁播放器，音乐持续；
// - 窗口关闭只隐藏到托盘，音乐同样持续。

import { reactive, readonly, watch } from 'vue';

interface FmMeta { artist: string; title: string; index: number }

/** 播放器共享状态（readonly 暴露给视图） */
const state = reactive({
  /** 后端引擎就绪后置 true：解锁播放按钮与托盘 FM 项 */
  ready: false,
  /** 播放/暂停（由后端引擎管理，通过 fm-state-changed 事件同步） */
  playing: false,
  /** 当前曲目元数据（由后端 fm-meta 事件驱动更新） */
  track: { artist: '', title: '', index: 0 } as { artist: string; title: string; index: number },
});

export const fmState = readonly(state);

// ── 初始化 ──

/** 启动时调用：获取初始状态 + 监听后端事件 */
async function init() {
  if (!('__TAURI_INTERNALS__' in window)) return;

  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');

  // 获取初始播放状态
  try {
    const ps = await invoke<FmMeta & { playing: boolean; ready: boolean }>('fm_get_state');
    state.ready = ps.ready;
    state.playing = ps.playing;
    state.track = { artist: ps.artist, title: ps.title, index: ps.index };
  } catch {
    // 后端未就绪时静默
  }

  // 监听后端切歌事件
  await listen<FmMeta>('fm-meta', (event) => {
    state.track = {
      artist: event.payload.artist,
      title: event.payload.title,
      index: event.payload.index,
    };
  });

  // 监听后端引擎就绪
  await listen<void>('fm-ready', () => {
    state.ready = true;
    invoke('fm_ready').catch(() => {});
  });

  // 监听后端播放状态变化（同步托盘勾选）
  await listen<boolean>('fm-state-changed', (event) => {
    state.playing = event.payload;
    invoke('fm_set_playing', { playing: event.payload }).catch(() => {});
  });
}

// ── 播放控制 ──

async function toggle() {
  if (!('__TAURI_INTERNALS__' in window)) return;
  const { invoke } = await import('@tauri-apps/api/core');
  invoke('fm_toggle').catch(() => {});
}

async function play() {
  if (!('__TAURI_INTERNALS__' in window)) return;
  const { invoke } = await import('@tauri-apps/api/core');
  invoke('fm_play').catch(() => {});
}

async function pause() {
  if (!('__TAURI_INTERNALS__' in window)) return;
  const { invoke } = await import('@tauri-apps/api/core');
  invoke('fm_pause').catch(() => {});
}

export const fmPlayer = { toggle };

export function initFm() {
  void init();
}
