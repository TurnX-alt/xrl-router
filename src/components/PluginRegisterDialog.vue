<template>
  <md-dialog :open="visible" @closed="onClosed">
    <div slot="headline">发现插件：{{ providerName }}</div>
    <form slot="content" method="dialog">
      <p class="desc">是否将该插件添加为供应商？</p>
    </form>
    <div slot="actions">
      <md-text-button @click="cancel">忽略</md-text-button>
      <md-filled-button @click="confirm">添加供应商</md-filled-button>
    </div>
  </md-dialog>
</template>

<script setup lang="ts">
import { ref } from 'vue';
import { useRouter } from 'vue-router';

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
      await fetch(`http://localhost:19068/api/plugins/${pluginId.value}`, {
        method: 'DELETE',
      });
    } catch (e) {
      console.error('忽略插件失败：', e);
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
