<template>
  <div v-if="!isOnline" class="connection-banner">
    <md-icon>cloud_off</md-icon>
    <span>无法连接到后端服务</span>
    <md-filled-button @click="retryConnection">重试</md-filled-button>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import { connectionState } from '../api.js';

const isOnline = ref(connectionState.isOnline);
let checkInterval: number | null = null;

const retryConnection = async () => {
  try {
    const response = await fetch('/health');
    if (response.ok) {
      connectionState.isOnline = true;
      isOnline.value = true;
    }
  } catch (err) {
    console.error('重试连接失败:', err);
  }
};

onMounted(() => {
  // 每 5 秒检查一次连接状态
  checkInterval = window.setInterval(() => {
    isOnline.value = connectionState.isOnline;
  }, 5000);
});

onUnmounted(() => {
  if (checkInterval !== null) {
    window.clearInterval(checkInterval);
    checkInterval = null;
  }
});
</script>

<style scoped>
.connection-banner {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  z-index: 9999;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 16px;
  background: var(--md-sys-color-error-container);
  color: var(--md-sys-color-on-error-container);
  font-size: 14px;
  font-weight: 500;
  box-shadow: var(--md-sys-elevation-2);
}

.connection-banner md-icon {
  font-size: 20px;
}

.connection-banner md-filled-button {
  --md-filled-button-container-color: var(--md-sys-color-error);
  --md-filled-button-label-text-color: var(--md-sys-color-on-error);
  margin-left: auto;
  font-size: 12px;
  height: 32px;
}
</style>
