import { defineStore } from 'pinia';
import { ref } from 'vue';
import { modelsApi, type Model } from '../api';

export type { Model };

export const useModelStore = defineStore('models', () => {
  const modelsByProvider = ref<Record<string, Model[]>>({});
  const loading = ref(false);

  async function fetchModels(providerId?: string) {
    loading.value = true;
    try {
      const models = await modelsApi.list(providerId);
      if (providerId) {
        modelsByProvider.value[providerId] = models;
      }
    } finally {
      loading.value = false;
    }
  }

  async function syncModels(providerId: string) {
    return modelsApi.sync(providerId);
  }

  async function addModel(data: any) {
    const model = await modelsApi.create(data);
    if (data.provider_id && !modelsByProvider.value[data.provider_id]) {
      modelsByProvider.value[data.provider_id] = [];
    }
    if (data.provider_id) {
      modelsByProvider.value[data.provider_id].push(model);
    }
    return model;
  }

  async function updateModel(modelId: string, data: any) {
    const model = await modelsApi.update(modelId, data);
    // Find and update in the appropriate provider bucket
    for (const pid of Object.keys(modelsByProvider.value)) {
      const models = modelsByProvider.value[pid];
      const idx = models.findIndex((m) => m.id === modelId);
      if (idx >= 0) { models[idx] = model; break; }
    }
    return model;
  }

  async function removeModel(modelId: string) {
    await modelsApi.delete(modelId);
    for (const pid of Object.keys(modelsByProvider.value)) {
      modelsByProvider.value[pid] = modelsByProvider.value[pid].filter((m) => m.id !== modelId);
    }
  }

  function getModels(providerId: string): Model[] {
    return modelsByProvider.value[providerId] || [];
  }

  return {
    modelsByProvider,
    loading,
    fetchModels,
    syncModels,
    addModel,
    updateModel,
    removeModel,
    getModels,
  };
});
