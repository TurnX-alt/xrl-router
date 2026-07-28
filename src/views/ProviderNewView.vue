<template>
  <div class="page">
    <h2 class="md-typescale-headline-medium page__title">{{ isEdit ? '维护供应商' : '添加供应商' }}</h2>

    <md-outlined-text-field
      :value="name"
      label="供应商名称"
      class="field"
      @input="name = ($event.target as HTMLInputElement).value"
    ></md-outlined-text-field>

    <md-outlined-select
      :value="kind"
      label="API 格式"
      class="field"
      menu-positioning="fixed"
      @input="kind = ($event.target as HTMLInputElement).value as 'openai' | 'anthropic'"
    >
      <md-select-option value="anthropic"><span slot="headline">Anthropic Messages</span></md-select-option>
      <md-select-option value="openai"><span slot="headline">OpenAI Chat Completions</span></md-select-option>
    </md-outlined-select>

    <md-outlined-text-field
      :value="baseUrl"
      label="Base URL"
      placeholder="https://api.example.com"
      class="field"
      @input="baseUrl = ($event.target as HTMLInputElement).value"
    ></md-outlined-text-field>

    <md-outlined-text-field
      :value="apiKeysText"
      type="textarea"
      rows="5"
      label="API Key"
      placeholder="sk-xxxxxxxx&#10;sk-yyyyyyyy"
      class="field"
      @input="apiKeysText = ($event.target as HTMLInputElement).value"
    ></md-outlined-text-field>

    <md-outlined-text-field
      :value="modelsText"
      type="textarea"
      rows="5"
      label="可用模型"
      placeholder="gpt-4o&#10;claude-opus-4-8&lt;-my-opus"
      class="field"
      @input="modelsText = ($event.target as HTMLInputElement).value"
    ></md-outlined-text-field>

    <div class="actions">
      <md-text-button @click="$router.push('/providers')">取消</md-text-button>
      <md-filled-button @click="save" :disabled="saving || !canSave">
        {{ saving ? '保存中...' : isEdit ? '保存修改' : '保存供应商' }}
      </md-filled-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useRouter, useRoute } from 'vue-router';
import { providersApi, keysApi, modelsApi } from '../api';

const router = useRouter();
const route = useRoute();

const editId = ref<string | null>(null);
const isEdit = computed(() => !!editId.value);

const name = ref('');
const baseUrl = ref('');
const kind = ref<'anthropic' | 'openai'>('anthropic');
const apiKeysText = ref('');
const modelsText = ref('');
const saving = ref(false);

// API Path 由 API 格式派生（不在界面显示）
const apiPathDerived = computed(() =>
  kind.value === 'anthropic' ? '/v1/messages' : '/v1/chat/completions',
);

const canSave = computed(() => name.value.trim() && baseUrl.value.trim());

function parseLines(text: string): string[] {
  return text
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);
}

function parseModelLine(line: string): { model_id: string; display_name: string } {
  const idx = line.indexOf('<-');
  if (idx >= 0) {
    const mid = line.slice(0, idx).trim();
    const alias = line.slice(idx + 2).trim();
    return { model_id: mid, display_name: alias || mid };
  }
  const mid = line.trim();
  return { model_id: mid, display_name: mid };
}

async function save() {
  const payload = {
    name: name.value.trim(),
    kind: kind.value,
    base_url: baseUrl.value.trim(),
    api_path: apiPathDerived.value,
    config: {},
  };
  saving.value = true;
  try {
    let providerId: string;

    if (isEdit.value) {
      await providersApi.update(editId.value!, payload);
      providerId = editId.value!;
    } else {
      const p = await providersApi.create(payload);
      providerId = p.id;
    }

    // API Keys（一行一个）：编辑时全量同步（删旧建新），创建时直接建
    if (isEdit.value) {
      const oldKeys = await keysApi.list(providerId);
      for (const k of oldKeys) {
        await keysApi.delete(providerId, k.id);
      }
    }
    for (const key of parseLines(apiKeysText.value)) {
      await keysApi.create(providerId, { name: payload.name + ' 密钥', key });
    }

    // 模型：编辑时全量同步（删旧建新），创建时直接建
    const models = parseLines(modelsText.value).map(parseModelLine);
    if (isEdit.value) {
      const oldModels = await modelsApi.list(providerId);
      for (const m of oldModels) {
        await modelsApi.delete(m.id);
      }
    }
    for (const m of models) {
      if (!m.model_id) continue;
      await modelsApi.create({
        provider_id: providerId,
        model_id: m.model_id,
        display_name: m.display_name,
        tier: 'custom',
      });
    }

    router.push('/providers');
  } catch (e: any) {
    alert(`保存失败：${e?.message || e}`);
  } finally {
    saving.value = false;
  }
}

onMounted(async () => {
  const id = route.params.id as string;
  if (!id) return;
  editId.value = id;
  try {
    const p = await providersApi.get(id);
    name.value = p.name;
    kind.value = (p.kind as 'anthropic' | 'openai') || 'anthropic';
    baseUrl.value = p.base_url || '';

    // 回填模型（display_name 与 model_id 不同时写成 model_id<-alias）
    const models = await modelsApi.list(id);
    modelsText.value = models
      .map((m: any) =>
        m.display_name && m.display_name !== m.model_id
          ? `${m.model_id}<-${m.display_name}`
          : m.model_id,
      )
      .join('\n');

    // 回填明文密钥
    const keys = await keysApi.list(id);
    if (keys.length) {
      apiKeysText.value = keys
        .map((k: any) => k.key_plain || '')
        .filter(Boolean)
        .join('\n');
    }
  } catch (e: any) {
    alert(`加载失败：${e?.message || e}`);
  }
});
</script>

<style scoped>
.page { max-width: 640px; }
.back-btn { display: inline-flex; align-items: center; gap: 4px; border: none; background: transparent; color: var(--md-sys-color-on-surface-variant); cursor: pointer; font-family: inherit; font-size: 0.875rem; padding: 0; margin-bottom: 12px; }
.back-btn:hover { color: var(--md-sys-color-on-surface); }
.page__title { margin: 0 0 24px; }
.field { width: 100%; margin-bottom: 16px; display: block; }
.actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 24px; }
</style>
