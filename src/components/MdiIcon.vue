<script setup lang="ts">
import { computed } from 'vue';
import * as mdiIcons from '@mdi/js';

const props = defineProps<{
  icon: string;  // kebab-case, 不含 mdi- 前缀, e.g. "plus"
}>();

const svgPath = computed(() => {
  const camel = 'mdi' + props.icon
    .split('-')
    .map(s => s[0].toUpperCase() + s.slice(1))
    .join('');
  return (mdiIcons as Record<string, string>)[camel] || '';
});
</script>

<template>
  <svg
    v-if="svgPath"
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    class="mdi-icon"
  >
    <path :d="svgPath" />
  </svg>
</template>

<style>
.mdi-icon {
  width: 1em;
  height: 1em;
  fill: currentColor;
  vertical-align: middle;
  max-width: 100%;
  max-height: 100%;
}
</style>
