<template>
  <div class="page">
    <div class="page__header">
      <h2 class="md-typescale-headline-medium page__title">密钥</h2>
      <md-filled-button @click="openCreate">
        <span slot="icon" class="mdi mdi-plus"></span>
        创建密钥
      </md-filled-button>
    </div>

    <div v-if="loading" class="empty-state"><md-circular-progress indeterminate></md-circular-progress></div>

    <div v-else-if="!keys.length" class="empty-state">
      <span class="mdi mdi-inbox-outline empty-state__icon"></span>
      <p class="md-typescale-body-large">空空如也</p>
    </div>

    <div v-else class="table-card">
      <table class="table">
        <thead>
          <tr class="md-typescale-label-large">
            <th>名称</th><th>密钥</th><th>可用模型</th><th>创建时间</th><th>修改时间</th><th></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="k in keys" :key="k.id" class="md-typescale-body-medium">
            <td class="name-cell">{{ k.name || '未命名' }}</td>
            <td class="mono">{{ k.key_masked }}</td>
            <td class="models-cell">
              <div class="models-inner">
                <span v-if="!k.allowed_models || !k.allowed_models.length" class="chip chip--all md-typescale-label-medium">全部</span>
                <span v-for="m in (k.allowed_models || [])" :key="m" class="chip md-typescale-label-medium">{{ m }}</span>
              </div>
            </td>
            <td class="time-cell">{{ formatTime(k.created_at) }}</td>
            <td class="time-cell">{{ formatTime(k.updated_at) }}</td>
            <td class="actions-cell">
              <md-icon-button :id="'key-btn-' + k.id" @click="toggleMenu(k)">
                <span class="mdi mdi-dots-vertical"></span>
              </md-icon-button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- Shared action menu (single instance, re-anchors per row) -->
    <md-menu
      :open="menuOpen != null"
      :anchor="menuAnchor"
      positioning="fixed"
      @closed="menuOpen = null"
    >
      <md-menu-item @click="renameFromMenu">
        <span class="mdi mdi-pencil-outline"></span> 重命名
      </md-menu-item>
      <md-menu-item @click="permFromMenu">
        <span class="mdi mdi-shield-outline"></span> 修改权限
      </md-menu-item>
      <md-menu-item class="menu-item--danger" @click="deleteFromMenu">
        <span class="mdi mdi-delete-outline"></span> 删除
      </md-menu-item>
    </md-menu>

    <md-dialog :open="createOpen" @close="createOpen = false">
      <div slot="headline">创建密钥</div>
      <div slot="content" class="form">
        <md-outlined-text-field :value="newName" label="备注名" class="field" @input="newName = ($event.target as HTMLInputElement).value"></md-outlined-text-field>
      </div>
      <div slot="actions">
        <md-text-button @click="createOpen = false">取消</md-text-button>
        <md-filled-button @click="createKey">创建</md-filled-button>
      </div>
    </md-dialog>

    <md-dialog :open="renameOpen" @close="renameOpen = false">
      <div slot="headline">重命名密钥</div>
      <div slot="content" class="form">
        <md-outlined-text-field :value="renameName" label="备注名" class="field" @input="renameName = ($event.target as HTMLInputElement).value"></md-outlined-text-field>
      </div>
      <div slot="actions">
        <md-text-button @click="renameOpen = false">取消</md-text-button>
        <md-filled-button @click="renameKey">保存</md-filled-button>
      </div>
    </md-dialog>

    <md-dialog :open="!!newKeyPlain" @close="newKeyPlain = ''">
      <div slot="headline">密钥已创建 — 仅显示一次</div>
      <div slot="content" class="form">
        <p class="warn md-typescale-body-medium"><span class="mdi mdi-alert"></span>请妥善保存，关闭后无法再次查看</p>
        <div class="key-box mono md-typescale-body-large">{{ newKeyPlain }}</div>
      </div>
      <div slot="actions">
        <md-text-button @click="copyKey"><span slot="icon" class="mdi mdi-content-copy"></span>复制</md-text-button>
        <md-filled-button @click="newKeyPlain = ''">完成</md-filled-button>
      </div>
    </md-dialog>

    <md-dialog :open="permOpen" @close="permOpen = false">
      <div slot="headline">权限管理 — {{ editingKey?.name || '未命名' }}</div>
      <div slot="content" class="form">
        <p class="md-typescale-body-medium perm-desc">按供应商区分可用模型。不勾选任何模型表示允许全部。</p>
        <md-circular-progress v-if="modelsLoading" indeterminate></md-circular-progress>
        <div v-else class="perm-list">
          <template v-for="p in providerModels" :key="p.name">
            <div class="perm-provider-label md-typescale-label-large">{{ p.name }}</div>
            <label v-for="m in p.models" :key="m" class="perm-item md-typescale-body-medium">
              <md-checkbox :checked="permSet.has(m)" @click="togglePerm(m)"></md-checkbox>
              {{ m }}
            </label>
          </template>
        </div>
        <p v-if="!allModels.length && !modelsLoading" class="md-typescale-body-medium">尚无供应商模型数据，请先在供应商页面配置。</p>
      </div>
      <div slot="actions">
        <md-text-button @click="permOpen = false">取消</md-text-button>
        <md-filled-button @click="savePerms">保存</md-filled-button>
      </div>
    </md-dialog>

    <md-dialog :open="deleteOpen" @close="deleteOpen = false">
      <div slot="headline">删除密钥</div>
      <div slot="content" class="form">
        <p class="md-typescale-body-medium">确定删除「{{ deleteTarget?.name || '未命名' }}」？此操作不可撤销。</p>
      </div>
      <div slot="actions">
        <md-text-button @click="deleteOpen = false">取消</md-text-button>
        <md-text-button class="confirm-del" @click="confirmDelete">确定删除</md-text-button>
      </div>
    </md-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { serviceKeysApi, providersApi, modelsApi, type ServiceKey } from '../api';

const keys = ref<ServiceKey[]>([]);
const loading = ref(true);
const createOpen = ref(false);
const newName = ref('');
const newKeyPlain = ref('');

const permOpen = ref(false);
const editingKey = ref<ServiceKey | null>(null);
const permSet = ref<Set<string>>(new Set());
const allModels = ref<string[]>([]);
const providerModels = ref<{ name: string; models: string[] }[]>([]);
const modelsLoading = ref(false);

// 共享操作菜单（单实例，按行重定向 anchor）
const menuOpen = ref<string | null>(null);
const menuAnchor = ref('');
const menuTarget = ref<ServiceKey | null>(null);
function toggleMenu(k: ServiceKey) {
  if (menuOpen.value === k.id) {
    menuOpen.value = null;
  } else {
    menuTarget.value = k;
    menuAnchor.value = 'key-btn-' + k.id;
    menuOpen.value = k.id;
  }
}
function renameFromMenu() {
  if (!menuTarget.value) return;
  editingKey.value = menuTarget.value;
  renameName.value = menuTarget.value.name || '';
  renameOpen.value = true;
}
function permFromMenu() {
  if (menuTarget.value) openPerms(menuTarget.value);
}
function deleteFromMenu() {
  if (menuTarget.value) openDeleteMenu(menuTarget.value);
}

const renameOpen = ref(false);
const renameName = ref('');
async function renameKey() {
  if (!editingKey.value) return;
  try {
    await serviceKeysApi.update(editingKey.value.id, { name: renameName.value.trim() || '未命名' });
    renameOpen.value = false;
    await fetchKeys();
  } catch (e: any) {
    alert(`重命名失败：${e?.message || e}`);
  }
}

async function fetchKeys() { loading.value = true; try { keys.value = await serviceKeysApi.list(); } finally { loading.value = false; } }
function openCreate() { newName.value = ''; createOpen.value = true; }
async function createKey() {
  try {
    const r = await serviceKeysApi.create({ name: newName.value || '未命名' });
    createOpen.value = false;
    newKeyPlain.value = r.key;
    await fetchKeys();
  } catch (e: any) {
    alert(`创建密钥失败：${e?.message || e}`);
  }
}
async function copyKey() { try { await navigator.clipboard.writeText(newKeyPlain.value); } catch {} }
const deleteOpen = ref(false);
const deleteTarget = ref<ServiceKey | null>(null);
function openDeleteMenu(k: ServiceKey) {
  deleteTarget.value = k;
  deleteOpen.value = true;
}
async function confirmDelete() {
  if (!deleteTarget.value) return;
  try {
    await serviceKeysApi.delete(deleteTarget.value.id);
    deleteOpen.value = false;
    deleteTarget.value = null;
    await fetchKeys();
  } catch (e: any) {
    alert(`删除失败：${e?.message || e}`);
  }
}

function openPerms(k: ServiceKey) {
  editingKey.value = k;
  permSet.value = new Set(k.allowed_models || []);
  permOpen.value = true;
  fetchAvailableModels();
}

async function fetchAvailableModels() {
  modelsLoading.value = true;
  try {
    const [providers, models] = await Promise.all([providersApi.list(), modelsApi.list()]);
    const providerName = new Map(providers.map((p) => [p.id, p.name]));
    const groupsMap = new Map<string, string[]>();
    for (const m of models) {
      const pname = providerName.get(m.provider_id) || '未知';
      const name = m.display_name || m.model_id;
      if (!groupsMap.has(pname)) groupsMap.set(pname, []);
      if (!groupsMap.get(pname)!.includes(name)) groupsMap.get(pname)!.push(name);
    }
    const groups = Array.from(groupsMap.entries()).map(([name, ms]) => ({ name, models: ms.sort() }));
    providerModels.value = groups;
    allModels.value = groups.flatMap((g) => g.models);
  } catch {} finally { modelsLoading.value = false; }
}

function togglePerm(model: string) {
  const s = new Set(permSet.value);
  s.has(model) ? s.delete(model) : s.add(model);
  permSet.value = s;
}

async function savePerms() {
  if (!editingKey.value) return;
  const models = [...permSet.value];
  try {
    await serviceKeysApi.update(editingKey.value.id, { name: editingKey.value.name || undefined, allowed_models: models });
    permOpen.value = false;
    await fetchKeys();
  } catch (e: any) {
    alert(`保存权限失败：${e?.message || e}`);
  }
}

function formatTime(t: number): string { const d = new Date(t*1000); return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')} ${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}`; }
onMounted(fetchKeys);
</script>

<style scoped>
.page__header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 24px; gap: 16px; flex-wrap: wrap; }
.page__title { margin: 0; }
.empty-state { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 64px 24px; text-align: center; }
.empty-state__icon { font-size: 48px; color: var(--md-sys-color-on-surface-variant); }
.table-card { background: var(--md-sys-color-surface-container-low); border-radius: var(--md-sys-shape-corner-medium); padding: 16px; }
.table { width: 100%; border-collapse: collapse; table-layout: fixed; }
.table th { text-align: left; padding: 12px 16px; color: var(--md-sys-color-on-surface-variant); vertical-align: middle; }
.table td { padding: 12px 16px; vertical-align: middle; }
.table tr { border-bottom: 1px solid var(--md-sys-color-outline-variant); }
.table tr:last-child { border-bottom: none; }
.name-cell { width: 24%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.models-cell { width: auto; }
.models-inner { display: inline-flex; flex-wrap: wrap; gap: 4px 6px; align-items: center; vertical-align: middle; }
.time-cell { width: 14%; white-space: nowrap; color: var(--md-sys-color-on-surface-variant); }
.actions-cell { width: 56px; text-align: right; }
.mono { font-family: 'Roboto Mono', monospace; }
.chip { display: inline-flex; align-items: center; padding: 2px 8px; border-radius: var(--md-sys-shape-corner-small); background: var(--md-sys-color-primary-container); color: var(--md-sys-color-on-primary-container); font-size: 0.75rem; line-height: 1.4; }
.chip--all { background: var(--md-sys-color-surface-container-highest); color: var(--md-sys-color-on-surface-variant); }
.form { display: flex; flex-direction: column; gap: 16px; min-width: 360px; }
.field { width: 100%; }
.warn { display: flex; align-items: center; gap: 8px; color: var(--md-sys-color-on-error-container); background: var(--md-sys-color-error-container); padding: 12px; border-radius: var(--md-sys-shape-corner-small); margin: 0; }
.key-box { background: var(--md-sys-color-surface-container-high); padding: 16px; border-radius: var(--md-sys-shape-corner-medium); word-break: break-all; border: 1px solid var(--md-sys-color-outline-variant); }
.perm-desc { color: var(--md-sys-color-on-surface-variant); margin: 0; }
.perm-list { max-height: 300px; overflow-y: auto; display: flex; flex-direction: column; gap: 4px; }
.perm-provider-label { color: var(--md-sys-color-on-surface-variant); padding: 8px 0 4px; border-top: 1px solid var(--md-sys-color-outline-variant); margin-top: 4px; }
.perm-provider-label:first-child { border-top: none; margin-top: 0; }
.perm-item { display: flex; align-items: center; gap: 8px; cursor: pointer; padding: 6px 0 6px 8px; }
.perm-item md-checkbox { flex-shrink: 0; margin-top: -2px; }
</style>

<!-- md-menu teleports to document root, so its styles must not be scoped -->
<style>
.menu-item--danger { --md-menu-item-label-text-color: var(--md-sys-color-error); color: var(--md-sys-color-error); }
</style>