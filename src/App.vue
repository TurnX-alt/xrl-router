<template>
  <ConnectionStatus v-if="!isInstall" />
  <AppShell v-if="!isInstall">
    <router-view />
  </AppShell>
  <router-view v-if="isInstall" />
  <PluginRegisterDialog ref="pluginDialogRef" />
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useRoute } from 'vue-router';
import AppShell from './components/AppShell.vue';
import ConnectionStatus from './components/ConnectionStatus.vue';
import PluginRegisterDialog from './components/PluginRegisterDialog.vue';

const pluginDialogRef = ref<InstanceType<typeof PluginRegisterDialog> | null>(null);

const route = useRoute();
const isInstall = computed(() => route.path === '/install');

onMounted(async () => {
  // 非 Tauri 环境（LAN 浏览器）跳过原生事件监听
  if (!('__TAURI_INTERNALS__' in window)) return;

  const { listen } = await import('@tauri-apps/api/event');

  // 监听插件注册事件
  await listen('plugin-register', (event: any) => {
    console.log('[Plugin] Register event:', event.payload);
    pluginDialogRef.value?.show(event.payload);
  });

  // 监听插件离线事件
  await listen('plugin-offline', (event: any) => {
    console.log('[Plugin] Offline:', event.payload);
  });

  // 监听插件上线事件
  await listen('plugin-online', (event: any) => {
    console.log('[Plugin] Online:', event.payload);
  });
});
</script>