<template>
  <div class="page">
    <div class="page__header">
      <h2 class="md-typescale-headline-medium page__title">设置</h2>
    </div>

    <section class="card section">
      <div class="section__head">
        <span class="section__icon mdi mdi-information-outline"></span>
        <div>
          <h3 class="md-typescale-title-medium">关于</h3>
          <p class="md-typescale-body-medium section__desc">
            XRL Router 是一个多 Provider AI LLM API 路由网关，支持跨 Provider 的模型层级调度和协议翻译。
          </p>
          <a class="section__link md-typescale-body-medium" :href="GITHUB_URL" @click.prevent="openExternal">
            <span class="mdi mdi-open-in-new"></span>
            github.com/wpy030414/xrl-router
          </a>
        </div>
      </div>
    </section>

    <section class="card section">
      <div class="section__head">
        <span class="section__icon mdi mdi-palette"></span>
        <div>
          <h3 class="md-typescale-title-medium">主题</h3>
          <p class="md-typescale-body-medium section__desc">切换浅色或深色外观</p>
        </div>
      </div>
      <div class="section__body theme-buttons">
        <md-outlined-button :class="{ 'theme-btn--active': theme === 'light' }" @click="chooseTheme('light')">
          <span slot="icon" class="mdi mdi-white-balance-sunny"></span>
          浅色
        </md-outlined-button>
        <md-outlined-button :class="{ 'theme-btn--active': theme === 'dark' }" @click="chooseTheme('dark')">
          <span slot="icon" class="mdi mdi-weather-night"></span>
          深色
        </md-outlined-button>
      </div>
    </section>

    <section class="card section">
      <div class="section__head">
        <span class="section__icon mdi mdi-magnify"></span>
        <div>
          <h3 class="md-typescale-title-medium">劫持 WebSearch</h3>
          <p class="md-typescale-body-medium section__desc">
            开启后，带 web_search 工具的请求由本地网页搜索包装处理，不经上游官方搜索
          </p>
        </div>
      </div>
      <div class="section__body switch-row">
        <md-switch :selected="hijack" @change="toggleHijack"></md-switch>
        <span class="md-typescale-body-medium switch-label">{{ hijack ? '已开启' : '已关闭' }}</span>
      </div>
    </section>

    <section class="card section section--danger">
      <div class="section__head">
        <span class="section__icon mdi mdi-delete-forever"></span>
        <div>
          <h3 class="md-typescale-title-medium">清除数据</h3>
          <p class="md-typescale-body-medium section__desc">清除所有本地存储的数据，此操作不可恢复</p>
        </div>
      </div>
      <div class="section__body">
        <md-outlined-button class="danger-btn" @click="destroy">
          <span slot="icon" class="mdi mdi-delete-forever"></span>
          清除所有本地数据
        </md-outlined-button>
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { settingsApi } from '../api';
import { getTheme, setTheme, type Theme } from '../theme';
import { open as openUrl } from '@tauri-apps/plugin-shell';

const router = useRouter();
const GITHUB_URL = 'https://github.com/wpy030414/xrl-router';

async function openExternal() {
  try {
    await openUrl(GITHUB_URL);
  } catch {
    window.open(GITHUB_URL, '_blank');
  }
}
const theme = ref<Theme>(getTheme());
const hijack = ref(false);

function chooseTheme(t: Theme) {
  theme.value = t;
  setTheme(t);
}

async function toggleHijack() {
  const next = !hijack.value;
  hijack.value = next;
  try {
    await settingsApi.update({ websearch_hijack: next });
  } catch {
    hijack.value = !next; // 失败回滚
  }
}

onMounted(async () => {
  try {
    const s = await settingsApi.get();
    hijack.value = !!s.websearch_hijack;
  } catch {
    // ignore
  }
});

function destroy() {
  if (!confirm('确定清除所有本地数据？此操作不可恢复。')) return;
  localStorage.clear();
  alert('本地数据已清除');
  router.push('/');
}
</script>

<style scoped>
.page__header { margin-bottom: 24px; }
.page__title { margin: 0; color: var(--md-sys-color-on-surface); }

.section {
  background: var(--md-sys-color-surface-container-low);
  border-radius: var(--md-sys-shape-corner-medium);
  padding: 20px;
  margin-bottom: 16px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.section__head { display: flex; align-items: flex-start; gap: 12px; }
.section__icon {
  width: 40px; height: 40px;
  border-radius: var(--md-sys-shape-corner-full);
  background: var(--md-sys-color-surface-container-high);
  color: var(--md-sys-color-on-surface-variant);
  display: flex; align-items: center; justify-content: center;
  font-size: 24px; flex-shrink: 0;
}
.section__head h3 { margin: 0; color: var(--md-sys-color-on-surface); }
.section__desc { margin: 2px 0 0; color: var(--md-sys-color-on-surface-variant); }

.section__link {
  display: inline-flex; align-items: center; gap: 6px;
  color: var(--md-sys-color-primary); text-decoration: none;
  margin-top: 2px;
}
.section__link:hover { text-decoration: underline; }

.section__body { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; }
.section--danger .section__icon { background: var(--md-sys-color-error-container); color: var(--md-sys-color-on-error-container); }
.danger-btn { color: var(--md-sys-color-error); }

.theme-buttons { gap: 12px; }
.theme-btn--active {
  --md-outlined-button-outline-color: var(--md-sys-color-primary);
  --md-outlined-button-label-text-color: var(--md-sys-color-primary);
  background: var(--md-sys-color-primary-container);
}

.switch-row { display: flex; align-items: center; gap: 12px; }
.switch-label { color: var(--md-sys-color-on-surface-variant); }
</style>
