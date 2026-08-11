<script setup lang="ts">
defineProps<{
  path: string;  // SVG path d 属性，由调用者从 @mdi/js 导入
}>();
</script>

<template>
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    class="mdi-icon"
  >
    <path :d="path" />
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
