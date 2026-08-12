<template>
  <md-dialog :open="visible" @closed="onClosed">
    <div slot="headline">{{ t('plugin.dialog.headline', { name: providerName }) }}</div>
    <form slot="content" method="dialog">
      <p class="desc">{{ t('plugin.dialog.desc') }}</p>
    </form>
    <div slot="actions">
      <md-text-button @click="cancel">{{ t('plugin.dialog.ignore') }}</md-text-button>
      <md-filled-button @click="confirm">{{ t('plugin.dialog.add') }}</md-filled-button>
    </div>
  </md-dialog>
</template>

<script setup lang="ts">
import { ref } from 'vue';
import { useRouter } from 'vue-router';
import { BASE_URL } from '../api';
import { t } from '../i18n';

const router = useRouter();

const visible = ref(false);
const providerName = ref('');
const pluginId = ref('');
const providerId = ref('');

function show(data: { plugin_id: string; provider_id: string; provider_name?: string }) {
  pluginId.value = data.plugin_id || '';
  providerId.value = data.provider_id || '';
  providerName.value = data.provider_name || data.plugin_id || '';
  visible.value = true;
}

async function confirm() {
  visible.value = false;
  // 跳转到 ProviderNewView，以插件模式展示完整表单
  router.push({
    path: '/providers/new',
    query: { plugin_id: pluginId.value, provider_id: providerId.value },
  });
}

async function cancel() {
  visible.value = false;
  // 忽略 = 彻底删除：删除插件记录 + 关联 provider + 模型
  // 下次插件重连时会重新注册、重新弹窗
  if (pluginId.value) {
    try {
      await fetch(`${BASE_URL}/api/plugins/${pluginId.value}`, {
        method: 'DELETE',
      });
    } catch (e) {
      console.error(t('plugin.dialog.ignore'), e);
    }
  }
}

function onClosed() {
  // 仅关闭，不重置（可能再次打开）
}

defineExpose({ show });
</script>

<style scoped>
.desc { margin: 0; }
</style>
