<template>
  <div class="page">
    <div class="page__header">
      <h2 class="md-typescale-headline-medium page__title">统计</h2>
      <div class="date-picker">
        <button class="date-btn" id="date-btn" @click="showPicker = true">
          <span class="mdi" :class="rangeIcon"></span>
          {{ rangeLabel }}
        </button>
        <md-menu :open="showPicker" anchor="date-btn" positioning="fixed" @closed="showPicker = false">
          <md-menu-item @click="setRange('today')"><span class="mdi mdi-calendar-today"></span> 当天</md-menu-item>
          <md-menu-item @click="setRange('1d')"><span class="mdi mdi-calendar-range"></span> 一天内</md-menu-item>
          <md-menu-item @click="setRange('7d')"><span class="mdi mdi-calendar-week"></span> 一周内</md-menu-item>
          <md-menu-item @click="setRange('30d')"><span class="mdi mdi-calendar-month"></span> 一月内</md-menu-item>
        </md-menu>
      </div>
    </div>

    <!-- 数据磁贴 -->
    <div class="tiles">
      <div class="tile tile--wide">
        <div class="tile__icon mdi mdi-counter"></div>
        <div class="tile__content">
          <div class="tile__label">总消耗 Tokens</div>
          <div class="tile__value tile__value--big">
            {{ formatTokens(animTotalTokens) }}<span class="tile__value-sub">≈{{ formatTokensAuto(animTotalTokens) }}</span>
          </div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-star"></div>
        <div class="tile__content">
          <div class="tile__label">调用最多模型</div>
          <div class="tile__value">{{ topModelName }}</div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-api"></div>
        <div class="tile__content">
          <div class="tile__label">总请求次</div>
          <div class="tile__value">{{ formatTokens(animTotalRequests) }}</div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-arrow-down-bold"></div>
        <div class="tile__content">
          <div class="tile__label">输入 Tokens</div>
          <div class="tile__value">{{ formatTokensAuto(animTotalInputTokens) }}</div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-arrow-up-bold"></div>
        <div class="tile__content">
          <div class="tile__label">输出 Tokens</div>
          <div class="tile__value">{{ formatTokensAuto(animTotalOutputTokens) }}</div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-database-search"></div>
        <div class="tile__content">
          <div class="tile__label">命中 Tokens</div>
          <div class="tile__value">{{ formatTokensAuto(animTotalCacheTokens) }}</div>
        </div>
      </div>
      <div class="tile">
        <div class="tile__icon mdi mdi-percent"></div>
        <div class="tile__content">
          <div class="tile__label">缓存命中率</div>
          <div class="tile__value">{{ animCacheHitRate }}%</div>
        </div>
      </div>
    </div>

    <div class="chart-container table-card">
      <Line :data="chartData" :options="chartOptions" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import { Line } from 'vue-chartjs';
import {
  Chart as ChartJS, CategoryScale, LinearScale, PointElement, LineElement,
  Title, Tooltip, Legend, Filler
} from 'chart.js';
import { statsApi } from '../api';
import { wsClient } from '../ws';

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Title, Tooltip, Legend, Filler);

interface StatRow { key_id: string; key_label?: string; prompt_tokens: number; completion_tokens: number; cache_read_input_tokens?: number; total_tokens: number; requests: number; day: string; }
interface TopModel { model_id: string; model_name: string; prompt_tokens: number; completion_tokens: number; cache_read_input_tokens: number; total_tokens: number; requests: number; }
const data = ref<StatRow[]>([]);
const topModel = ref<TopModel | null>(null);
const rangeFrom = ref(0);
const rangeTo = ref(0);
const rangeGranularity = ref<'hour' | 'day'>('day');
// 系统本地时区偏移（秒），UTC+8 = 28800；用于把桶对齐到本地天/小时边界，而非 UTC。
const tzOffset = -new Date().getTimezoneOffset() * 60;

// X 轴始终是所选范围内的完整连续序列（按当前粒度分桶，与后端一致），
// 无数据的桶也保留并填 0 —— 否则真实用量稀疏时坐标轴会出现空洞或停在最后一条记录。
const dates = computed(() => {
  if (!rangeFrom.value || !rangeTo.value) return [];
  const step = rangeGranularity.value === 'hour' ? 3600 : 86400;
  const prefix = rangeGranularity.value === 'hour' ? 'h' : 'd';
  const start = Math.floor((rangeFrom.value + tzOffset) / step);
  const end = Math.floor((rangeTo.value + tzOffset) / step);
  const out: string[] = [];
  for (let b = start; b <= end; b++) out.push(`${prefix}${b}`);
  return out;
});
// 真实出现过的服务密钥去重，用作图例系列；无数据时为空，图表显示空白。
const keys = computed(() => [...new Set(data.value.map(d => d.key_id || ''))].filter(Boolean).sort());
// key_id -> 可读标签（密钥名 + 掩码），后端已 JOIN api_keys 给出；用于图例显示。
const keyLabelMap = computed(() => {
  const m: Record<string, string> = {};
  for (const d of data.value) {
    if (d.key_id && !(d.key_id in m)) m[d.key_id] = d.key_label || d.key_id;
  }
  return m;
});

// Build per-key-per-model data for hover drilldown
const drillData = computed(() => {
  const map: Record<string, Record<string, number>> = {};
  for (const row of data.value) {
    const kid = row.key_id || '(all)';
    if (!map[`${row.day}|${kid}`]) map[`${row.day}|${kid}`] = {};
    map[`${row.day}|${kid}`]['total'] = (map[`${row.day}|${kid}`]['total'] || 0) + row.total_tokens;
  }
  return map;
});

function mdVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

const chartData = computed(() => {
  const colors = [
    mdVar('--md-sys-color-primary') || '#6750A4',
    mdVar('--md-sys-color-error') || '#B3261E',
    mdVar('--md-sys-color-tertiary') || '#7D5260',
    mdVar('--md-sys-color-secondary') || '#625B71',
    mdVar('--md-sys-color-tertiary') || '#7D5260',
    mdVar('--md-sys-color-inverse-primary') || '#D0BCFF',
  ];
  const datasets = keys.value.map((kid, i) => ({
    label: keyLabelMap.value[kid] || kid,
    data: dates.value.map(day => {
      const t = data.value.filter(d => d.day === day && d.key_id === kid).reduce((s, d) => s + d.total_tokens, 0);
      return t; // raw total tokens, display as 万 in tooltip only
    }),
    borderColor: colors[i % colors.length],
    backgroundColor: colors[i % colors.length] + '20',
    fill: false,
    tension: 0.35,
    spanGaps: true,
    pointRadius: 2,
    pointHoverRadius: 6,
  }));
  return { labels: dates.value, datasets };
});

const chartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: { mode: 'index' as const, intersect: false },
  plugins: {
    legend: { position: 'bottom' as const, labels: { usePointStyle: true, padding: 16, font: { family: 'Roboto Flex', size: 12 } } },
    tooltip: {
      callbacks: {
        title: function (ctx: any) {
          const label = ctx[0]?.label || '';
          return formatBucketLong(label);
        },
        label: function (ctx: any) {
          const kidLabel = ctx.dataset.label;
          const raw = Number(ctx.raw);
          return `${kidLabel}: ${(raw / 10000).toFixed(1)}万 Token`;
        },
        afterBody: function (ctx: any) {
          const day = ctx[0]?.label;
          const kidLabel = ctx[0]?.dataset?.label || '';
          const kid = keys.value.find(k => (keyLabelMap.value[k] || k) === kidLabel) || '';
          const dayData = data.value.filter(d => d.day === day && d.key_id === kid);
          if (dayData.length === 0) return [];
          const lines: string[] = ['Token分布:'];
          for (const d of dayData) {
            const cacheR = (d.cache_read_input_tokens || 0) / 10000;
            const input = (d.prompt_tokens / 10000).toFixed(1);
            const output = (d.completion_tokens / 10000).toFixed(1);
            let line = `  合计: ${(d.total_tokens / 10000).toFixed(1)}万 (输入 ${input}万 + 输出 ${output}万`;
            if (cacheR > 0.05) {
              line += ` | 缓存读 ${cacheR.toFixed(1)}万`;
            }
            line += ')';
            lines.push(line);
          }
          return lines;
        }
      }
    }
  },
  scales: {
    x: {
      grid: { display: false },
      title: { display: true, text: '时间', font: { family: 'Roboto Flex', size: 12 } },
      ticks: {
        font: { family: 'Roboto Flex', size: 10 },
        maxTicksLimit: 7,
        callback: function(_value: any, index: number) {
          const dayCount = dates.value.length;
          // Longer ranges: show every Nth label so they don't overlap.
          if (dayCount > 7) {
            const step = Math.ceil(dayCount / 7);
            if (index % step !== 0) return '';
          }
          const label = dates.value[index];
          return label ? formatBucket(label) : '';
        }
      }
    },
    y: {
      beginAtZero: true,
      min: 0,
      title: { display: true, text: 'Token (万)', font: { family: 'Roboto Flex', size: 12 } },
      ticks: {
        font: { family: 'Roboto Flex', size: 10 },
        maxTicksLimit: 6,
        callback: function(value: any) { return (Number(value) / 10000).toFixed(0); }
      }
    }
  }
}));

// Date range
const range = ref<'today' | '1d' | '7d' | '30d'>('today');
const showPicker = ref(false);

const rangeIcon = computed(() => {
  if (range.value === 'today') return 'mdi-calendar-today';
  if (range.value === '1d') return 'mdi-calendar-range';
  if (range.value === '7d') return 'mdi-calendar-week';
  return 'mdi-calendar-month';
});

const rangeLabel = computed(() => {
  if (range.value === 'today') return '当天';
  if (range.value === '1d') return '一天内';
  if (range.value === '7d') return '一周内';
  return '一月内';
});

function setRange(r: 'today' | '1d' | '7d' | '30d') {
  range.value = r;
  showPicker.value = false;
  fetchStats();
}

const customFrom = ref('');
const customTo = ref('');

async function fetchStats() {
  const now = Math.floor(Date.now() / 1000);
  let granularity: 'hour' | 'day';
  if (range.value === 'today') {
    // 从今日 0 点（含）到现在，按小时分桶
    const dayNow = Math.floor((now + tzOffset) / 86400);
    rangeFrom.value = dayNow * 86400 - tzOffset;
    rangeTo.value = now;
    granularity = 'hour';
  } else if (range.value === '1d') {
    // 24 个小时桶，最右刻度 = 当前小时桶的起始（如 01:49 → 最右刻度 01:00）；
    // 当前小时数据查到 now，落在最右桶内。
    const hourNow = Math.floor((now + tzOffset) / 3600);
    rangeTo.value = hourNow * 3600 - tzOffset;
    rangeFrom.value = rangeTo.value - 23 * 3600;
    granularity = 'hour';
  } else {
    // N 个天桶，最右刻度 = 当天 0 点（如 7/31 → 最右刻度 7/31）；
    // 当天数据查到 now，落在最右桶内。
    const days = range.value === '7d' ? 7 : 30;
    const dayNow = Math.floor((now + tzOffset) / 86400);
    rangeTo.value = dayNow * 86400 - tzOffset;
    rangeFrom.value = rangeTo.value - (days - 1) * 86400;
    granularity = 'day';
  }
  rangeGranularity.value = granularity;
  try {
    const json = await statsApi.query({ from: rangeFrom.value, to: now, granularity, tz_offset: tzOffset });
    data.value = json?.data ?? [];
    topModel.value = json?.top_model ?? null;
  } catch {
    data.value = [];
    topModel.value = null;
  }
}

// 磁贴聚合：基于当前所选时间范围，对所有桶求和
const totalTokens = computed(() => data.value.reduce((s, d) => s + d.total_tokens, 0));
const totalInputTokens = computed(() => data.value.reduce((s, d) => s + d.prompt_tokens, 0));
const totalOutputTokens = computed(() => data.value.reduce((s, d) => s + d.completion_tokens, 0));
const totalCacheTokens = computed(() => data.value.reduce((s, d) => s + (d.cache_read_input_tokens || 0), 0));
const totalRequests = computed(() => data.value.reduce((s, d) => s + d.requests, 0));
// 缓存命中率 = 命中 Tokens / (未缓存输入 + 命中 Tokens)
// 后端将 prompt_tokens 拆为「未命中输入」与 cache_read_input_tokens（命中），
// 两者互斥且相加才是全部输入，因此分母必须包含命中部分，否则会 >100%。
const cacheHitRate = computed(() => {
  const input = totalInputTokens.value + totalCacheTokens.value;
  if (input <= 0) return '0.0';
  return ((totalCacheTokens.value / input) * 100).toFixed(1);
});
const topModelName = computed(() => topModel.value?.model_name || '—');

// 数字翻动动画：平滑过渡到新值
function useAnimatedNumber(source: () => number) {
  const displayed = ref(0);
  let animationId: number | null = null;
  const DURATION = 2400; // ms

  watch(source, (newVal) => {
    if (animationId) cancelAnimationFrame(animationId);
    const startVal = displayed.value;
    const startTime = performance.now();

    function tick(now: number) {
      const elapsed = now - startTime;
      const progress = Math.min(elapsed / DURATION, 1);
      // easeOutSine: 前 1/3 快速翻动，后 2/3 缓慢趋近目标值
      const ease = Math.sin(progress * Math.PI / 2);
      displayed.value = Math.round(startVal + (newVal - startVal) * ease);
      if (progress < 1) {
        animationId = requestAnimationFrame(tick);
      }
    }
    animationId = requestAnimationFrame(tick);
  }, { immediate: true });

  return displayed;
}

const animTotalTokens = useAnimatedNumber(() => totalTokens.value);
const animTotalInputTokens = useAnimatedNumber(() => totalInputTokens.value);
const animTotalOutputTokens = useAnimatedNumber(() => totalOutputTokens.value);
const animTotalCacheTokens = useAnimatedNumber(() => totalCacheTokens.value);
const animTotalRequests = useAnimatedNumber(() => totalRequests.value);

// 缓存命中率动画（保留1位小数）
const animCacheHitRate = computed(() => {
  const input = animTotalInputTokens.value + animTotalCacheTokens.value;
  if (input <= 0) return '0.0';
  return ((animTotalCacheTokens.value / input) * 100).toFixed(1);
});

// 总消耗 Tokens：不换算，直接显示整数（带千位分隔符）
function formatTokens(n: number): string {
  return Math.round(n).toLocaleString();
}
// 输入/输出/命中：自动以万/亿结尾，保留 2 位小数
function formatTokensAuto(n: number): string {
  const yi = n / 1e8;
  if (yi >= 1) return `${yi.toFixed(2)}亿`;
  const wan = n / 1e4;
  return `${wan.toFixed(2)}万`;
}

function formatBucket(label: string): string {
  const m = label.match(/^([hd])(\d+)$/);
  if (!m) return label;
  const step = m[1] === 'h' ? 3600 : 86400;
  const secs = parseInt(m[2]) * step - tzOffset;
  const d = new Date(secs * 1000);
  if (m[1] === 'h') return `${String(d.getHours()).padStart(2, '0')}:00`;
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

function formatBucketLong(label: string): string {
  const m = label.match(/^([hd])(\d+)$/);
  if (!m) return label;
  const step = m[1] === 'h' ? 3600 : 86400;
  const secs = parseInt(m[2]) * step - tzOffset;
  const d = new Date(secs * 1000);
  if (m[1] === 'h') return `${d.getMonth() + 1}/${d.getDate()} ${String(d.getHours()).padStart(2, '0')}:00`;
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

// 后端每 5s 通过 WS 推送 usage_stats_changed；收到后用当前参数重新拉取。
function onStatsChanged() {
  fetchStats();
}

onMounted(() => {
  fetchStats();
  wsClient.connect();
  wsClient.on('usage_stats_changed', onStatsChanged);
});

onUnmounted(() => {
  wsClient.off('usage_stats_changed', onStatsChanged);
});
</script>

<style scoped>
.page__header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 24px; gap: 16px; flex-wrap: wrap; }
.page__title { margin: 0; }
.date-picker { position: relative; }
.date-btn {
  display: inline-flex; align-items: center; gap: 8px;
  height: 40px; padding: 0 20px;
  border: 1px solid var(--md-sys-color-outline);
  border-radius: var(--md-sys-shape-corner-full);
  background: transparent;
  color: var(--md-sys-color-on-surface);
  font-family: inherit; font-size: 0.875rem; font-weight: 500;
  cursor: pointer;
  transition: background 150ms;
}
.date-btn:hover { background: var(--md-sys-color-surface-container-high); }

/* 数据磁贴网格：4 列，不足 4 列的自动换行 */
.tiles {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 12px;
  margin-bottom: 16px;
}
.tile {
  display: flex; align-items: center; gap: 12px;
  padding: 12px 16px;
  background: var(--md-sys-color-surface-container-low);
  border-radius: var(--md-sys-shape-corner-medium);
  border: 1px solid var(--md-sys-color-outline-variant, rgba(0,0,0,0.08));
  min-height: 64px;
}
.tile__icon {
  font-size: 22px;
  color: var(--md-sys-color-primary);
  flex-shrink: 0;
}
.tile__content { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tile__label {
  font-size: 0.75rem;
  color: var(--md-sys-color-on-surface-variant);
  letter-spacing: 0.02em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tile__value {
  font-size: 1.1rem;
  font-weight: 600;
  color: var(--md-sys-color-on-surface);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 总消耗 Tokens：大数字 + 小号换算后缀 */
.tile__value--big {
  font-size: 2rem;
  line-height: 1.2;
}
.tile__value-sub {
  margin-left: 8px;
  font-size: 0.8rem;
  font-weight: 400;
  color: var(--md-sys-color-on-surface-variant);
  vertical-align: 0.25em;
}
/* 总消耗 Tokens 独占两列 */
.tile--wide { grid-column: span 2; }

@media (max-width: 900px) {
  .tiles { grid-template-columns: repeat(2, 1fr); }
  .tile--wide { grid-column: span 2; }
}
@media (max-width: 520px) {
  .tiles { grid-template-columns: 1fr; }
  .tile--wide { grid-column: span 1; }
}

.chart-container { padding: 24px; background: var(--md-sys-color-surface-container-low); border-radius: var(--md-sys-shape-corner-medium); height: 440px; }
.chart-container canvas { max-height: 100% !important; }

.empty-state { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 64px 24px; text-align: center; }
.empty-state__icon { font-size: 48px; color: var(--md-sys-color-on-surface-variant); }

.table-card { background: var(--md-sys-color-surface-container-low); border-radius: var(--md-sys-shape-corner-medium); }
</style>