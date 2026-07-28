<template>
  <ConnectionStatus />
  <AppShell>
    <router-view />
  </AppShell>
</template>

<script setup lang="ts">
import AppShell from './components/AppShell.vue';
import ConnectionStatus from './components/ConnectionStatus.vue';
</script>