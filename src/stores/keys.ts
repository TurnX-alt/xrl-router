import { defineStore } from 'pinia';
import { ref } from 'vue';
import { keysApi } from '../api';

export interface ApiKey {
  id: string;
  provider_id: string;
  name: string;
  key_masked: string;
  status: 'green' | 'yellow' | 'red' | 'unknown';
  last_error: string | null;
  last_error_code: number | null;
  last_used_at: number | null;
  balance: number | null;
  total_requests: number;
  total_tokens: number;
}

export const useKeyStore = defineStore('keys', () => {
  const keysByProvider = ref<Record<string, ApiKey[]>>({});
  const loading = ref(false);

  async function fetchKeys(providerId?: string) {
    loading.value = true;
    try {
      const keys = await keysApi.list(providerId);
      if (providerId) {
        keysByProvider.value[providerId] = keys;
      }
    } finally {
      loading.value = false;
    }
  }

  async function addKey(providerId: string, data: { name: string; key: string }) {
    const key = await keysApi.create(providerId, data);
    if (!keysByProvider.value[providerId]) {
      keysByProvider.value[providerId] = [];
    }
    keysByProvider.value[providerId].push(key);
    return key;
  }

  async function removeKey(providerId: string, keyId: string) {
    await keysApi.delete(providerId, keyId);
    if (keysByProvider.value[providerId]) {
      keysByProvider.value[providerId] = keysByProvider.value[providerId].filter((k) => k.id !== keyId);
    }
  }

  function getKeys(providerId: string): ApiKey[] {
    return keysByProvider.value[providerId] || [];
  }

  return {
    keysByProvider,
    loading,
    fetchKeys,
    addKey,
    removeKey,
    getKeys,
  };
});
