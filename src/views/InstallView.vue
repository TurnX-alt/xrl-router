<template>
  <div class="page install-page">
    <div class="page__header">
      <h2 class="md-typescale-headline-medium page__title">{{ t('install.title') }}</h2>
    </div>

    <!-- 无密钥占位 -->
    <div v-if="!token" class="empty-state">
      <MdiIcon :path="mdiKeyRemove" class="empty-state__icon" />
      <p class="md-typescale-title-medium">{{ t('install.no_key_title') }}</p>
      <p class="md-typescale-body-medium muted">{{ t('install.no_key_desc') }}</p>
    </div>

    <!-- 有密钥：完整配置流程 -->
    <template v-else>
      <!-- 操作系统 -->
      <section class="card section">
        <div class="section__head">
          <span class="section__icon"><MdiIcon :path="mdiMonitor" /></span>
          <div>
            <h3 class="md-typescale-title-medium">{{ t('install.platform_label') }}</h3>
          </div>
        </div>
        <div class="section__body">
          <md-outlined-segmented-button-set>
            <md-outlined-segmented-button
              no-checkmark
              :selected="platform === 'unix'"
              label="macOS"
              @click="platform = 'unix'"
            ></md-outlined-segmented-button>
            <md-outlined-segmented-button
              no-checkmark
              :selected="platform === 'win'"
              label="Windows"
              @click="platform = 'win'"
            ></md-outlined-segmented-button>
          </md-outlined-segmented-button-set>
        </div>
      </section>

      <!-- 消费端 -->
      <section class="card section">
        <div class="section__head">
          <span class="section__icon"><MdiIcon :path="mdiAccount" /></span>
          <div>
            <h3 class="md-typescale-title-medium">{{ t('install.consumer_label') }}</h3>
          </div>
        </div>
        <div class="section__body">
          <md-outlined-segmented-button-set>
            <md-outlined-segmented-button
              no-checkmark
              :selected="mode === 'claude-code'"
              :label="t('install.mode_claude_code')"
              @click="mode = 'claude-code'"
            ></md-outlined-segmented-button>
            <md-outlined-segmented-button
              no-checkmark
              :selected="mode === 'chatgpt'"
              :label="t('install.mode_chatgpt')"
              @click="mode = 'chatgpt'"
            ></md-outlined-segmented-button>
          </md-outlined-segmented-button-set>
        </div>
      </section>

      <!-- 模型选择 -->
      <section class="card section">
        <div class="section__head">
          <span class="section__icon"><MdiIcon :path="mdiBrain" /></span>
          <div>
            <h3 class="md-typescale-title-medium">{{ t('install.default_model') }}</h3>
          </div>
        </div>
        <div class="section__body">
          <div v-if="modelsLoading" class="loading-row">
            <md-circular-progress indeterminate :diameter="20"></md-circular-progress>
            <span class="md-typescale-body-medium muted">{{ t('install.models_loading') }}</span>
          </div>
          <template v-else-if="modelsError">
            <p class="md-typescale-body-medium error-text">{{ modelsError }}</p>
            <p class="md-typescale-body-medium muted">{{ t('install.models_error_ignore') }}</p>
          </template>
          <p v-else-if="!models.length" class="md-typescale-body-medium muted">{{ t('install.no_models') }}</p>
          <md-outlined-select
            v-else
            :value="selectedModel"
            :label="t('install.default_model')"
            class="model-select"
            menu-positioning="fixed"
            @input="selectedModel = ($event.target as HTMLInputElement).value"
          >
            <md-select-option v-for="m in models" :key="m.id" :value="m.id">
              <span slot="headline">{{ m.id }}{{ m.owned_by ? ' · ' + m.owned_by : '' }}</span>
            </md-select-option>
          </md-outlined-select>
        </div>
      </section>

      <!-- 可执行命令 -->
      <section class="card section">
        <div class="section__head">
          <span class="section__icon"><MdiIcon :path="mdiConsole" /></span>
          <div>
            <h3 class="md-typescale-title-medium">{{ t('install.command_title', { platform: platformLabel }) }}</h3>
          </div>
        </div>
        <div class="section__body">
          <pre class="cmd-box">{{ command }}</pre>
        </div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import '@material/web/labs/segmentedbutton/outlined-segmented-button.js';
import '@material/web/labs/segmentedbuttonset/outlined-segmented-button-set.js';
import '@material/web/select/outlined-select.js';
import '@material/web/select/select-option.js';
import '@material/web/progress/circular-progress.js';
import { mdiKeyRemove, mdiMonitor, mdiAccount, mdiBrain, mdiConsole } from '@mdi/js';
import { t, i18n, setLocale } from '../i18n';
import type { Locale } from '../i18n';
import { setTheme, setHue } from '../theme';
import { uiSettingsApi } from '../api';
import MdiIcon from '../components/MdiIcon.vue';

// ── URL 参数 ──
const params = new URLSearchParams(location.search);
const token = params.get('t') || '';
const base = location.origin;

// ── 从后端读取管理端的 UI 设置（主题/令牌色/语言） ──
async function loadUiSettings() {
  try {
    const settings = await uiSettingsApi.get();
    // 应用主题
    if (settings.theme) {
      setTheme(settings.theme as any);
    }
    // 应用令牌色
    if (typeof settings.hue === 'number') {
      setHue(settings.hue);
    }
    // 应用语言
    if (settings.locale && (settings.locale === 'zh-CN' || settings.locale === 'en')) {
      setLocale(settings.locale);
    }
  } catch {
    // API 不可用，fallback 到 URL ?lang= 或浏览器语言
    const urlLang = params.get('lang');
    if (urlLang === 'zh-CN' || urlLang === 'en') {
      setLocale(urlLang);
    } else {
      const browserLang = (navigator.language || '').toLowerCase();
      setLocale(browserLang.startsWith('en') ? 'en' : 'zh-CN');
    }
  }
}

// ── 状态 ──
const platform = ref<'win' | 'unix'>(/Windows/i.test(navigator.userAgent) ? 'win' : 'unix');
const mode = ref<'claude-code' | 'chatgpt'>('claude-code');

const models = ref<{ id: string; owned_by: string; tier: string }[]>([]);
const selectedModel = ref('');
const modelsLoading = ref(true);
const modelsError = ref('');

const SLOTS = [{ slot: 'FABLE' }, { slot: 'HAIKU' }, { slot: 'OPUS' }, { slot: 'SONNET' }];

const platformLabel = computed(() =>
  platform.value === 'win' ? 'Windows (PowerShell)' : 'macOS (Bash)'
);

// ── 拉取模型 ──
async function fetchModels() {
  modelsLoading.value = true;
  modelsError.value = '';
  try {
    const r = await fetch(`${base}/v1/models`, { headers: { 'x-api-key': token } });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const data = await r.json();
    const list = (data?.data || []) as any[];
    models.value = list.map((m: any) => ({
      id: m.id,
      owned_by: m.owned_by || '',
      tier: m.tier || '',
    }));
    if (models.value.length) selectedModel.value = models.value[0].id;
  } catch (e: any) {
    modelsError.value = t('install.models_fetch_error', { msg: e.message || String(e) });
  } finally {
    modelsLoading.value = false;
  }
}

// ── 命令生成 ──
function q(v: string) { return `'${v}'`; }

function envModelLines(): string {
  const model = selectedModel.value || '';
  const lines = SLOTS.map(s =>
    `ANTHROPIC_DEFAULT_${s.slot}_MODEL=${q(model)}|ANTHROPIC_DEFAULT_${s.slot}_MODEL_NAME=${q(model)}`
  ).join('|');
  return `${lines}|CLAUDE_CODE_SUBAGENT_MODEL=${q(model)}`;
}

function psWriteSettings(): string {
  const envLines = envModelLines();
  const parts = [
    '$p="$env:USERPROFILE\\.claude\\settings.json"',
    'New-Item -ItemType Directory -Force "$env:USERPROFILE\\.claude" | Out-Null',
    '$j=@{}; if(Test-Path $p){ $j=Get-Content $p -Raw | ConvertFrom-Json }',
    'if(-not $j.env){ $j.env=@{} }',
    `$j.env.ANTHROPIC_AUTH_TOKEN='${token}'`,
    `$j.env.ANTHROPIC_BASE_URL='${base}'`,
  ];
  envLines.split('|').forEach(line => {
    const [k, ...rest] = line.split('=');
    parts.push(`$j.env.${k}=${rest.join('=')}`);
  });
  parts.push('$j | ConvertTo-Json -Depth 10 | Set-Content $p');
  return parts.join('; ');
}

function bashWriteSettings(): string {
  const envLines = envModelLines();
  let s = `const fs=require('fs'),p=process.env.HOME+'/.claude/settings.json';` +
    `let j={};try{j=JSON.parse(fs.readFileSync(p))}catch{};` +
    `j.env=j.env||{};` +
    `j.env.ANTHROPIC_AUTH_TOKEN=${q(token)};` +
    `j.env.ANTHROPIC_BASE_URL=${q(base)};`;
  envLines.split('|').forEach(line => {
    const [k, ...rest] = line.split('=');
    s += `j.env.${k}=${rest.join('=')};`;
  });
  s += 'fs.writeFileSync(p,JSON.stringify(j,null,2))';
  return `mkdir -p ~/.claude && node -e "${s}"`;
}

function buildClaudeCodeCommand(): string {
  return platform.value === 'win' ? psWriteSettings() : bashWriteSettings();
}

function buildChatGPTCommand(): string {
  const model = selectedModel.value || '';
  if (platform.value === 'win') {
    const tomlLines = [
      `model = '${model}'`,
      `model_provider = 'xrl'`,
      '',
      `[model_providers.xrl]`,
      `name = 'XRL Router'`,
      `base_url = '${base}/v1'`,
    ].join('`n');
    return [
      '$d="$env:USERPROFILE\\.codex"',
      'New-Item -ItemType Directory -Force $d | Out-Null',
      `Set-Content "$d\\config.toml" "${tomlLines}"`,
      `Set-Content "$d\\auth.json" '{"OPENAI_API_KEY":"${token}"}'`,
    ].join('; ');
  }

  return `mkdir -p ~/.codex && cat > ~/.codex/config.toml << 'CODEX_EOF'\n` +
    `model = "${model}"\n` +
    `model_provider = "xrl"\n\n` +
    `[model_providers.xrl]\n` +
    `name = "XRL Router"\n` +
    `base_url = "${base}/v1"\n` +
    `CODEX_EOF\n` +
    `printf '{"OPENAI_API_KEY":"${token}"}\\n' > ~/.codex/auth.json`;
}

const command = computed(() =>
  mode.value === 'chatgpt' ? buildChatGPTCommand() : buildClaudeCodeCommand()
);

onMounted(async () => {
  await loadUiSettings();
  if (token) fetchModels();
});
</script>

<style scoped>
.install-page {
  min-height: 100vh;
  padding: 32px;
  display: flex;
  flex-direction: column;
  align-items: center;
}

.install-page > * {
  width: 100%;
  max-width: 820px;
}

.page__header {
  margin-bottom: 24px;
}

.page__title {
  margin: 0;
  color: var(--md-sys-color-on-surface);
}

.subtitle {
  margin: 4px 0 0;
  color: var(--md-sys-color-on-surface-variant);
}

.section {
  background: var(--md-sys-color-surface-container-low);
  border-radius: var(--md-sys-shape-corner-medium);
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.section__head {
  display: flex;
  align-items: flex-start;
  gap: 12px;
}

.section__icon {
  width: 40px;
  height: 40px;
  border-radius: var(--md-sys-shape-corner-full);
  background: var(--md-sys-color-surface-container-high);
  color: var(--md-sys-color-on-surface-variant);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 24px;
  flex-shrink: 0;
}

.section__head h3 {
  margin: 0;
  color: var(--md-sys-color-on-surface);
}

.section__desc {
  margin: 4px 0 0;
  color: var(--md-sys-color-on-surface-variant);
}

.section__body {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.loading-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.model-select {
  min-width: 280px;
}

.cmd-box {
  background: var(--md-sys-color-surface-container-high);
  border-radius: var(--md-sys-shape-corner-small);
  padding: 16px;
  font-family: 'Roboto Mono', ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
  white-space: pre-wrap;
  word-break: break-all;
  line-height: 1.5;
  margin: 0;
  -webkit-user-select: text;
  user-select: text;
  color: var(--md-sys-color-on-surface);
}

.empty-state {
  text-align: center;
  padding: 48px 16px;
  color: var(--md-sys-color-on-surface-variant);
}

.empty-state__icon {
  font-size: 48px;
  margin-bottom: 12px;
  opacity: 0.6;
}

.muted {
  color: var(--md-sys-color-on-surface-variant);
}

.error-text {
  color: var(--md-sys-color-error);
}

.card {
  margin-bottom: 16px;
}
</style>
