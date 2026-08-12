import { defineStore } from 'pinia';
import { ref } from 'vue';
import { providersApi } from '../api';

export interface Provider {
  id: string;
  name: string;
  kind: 'messages' | 'chat_completions' | 'responses';
  base_url: string;
  enabled: boolean;
  created_at: number;
  updated_at: number;
}

export const useProviderStore = defineStore('providers', () => {
  const providers = ref<Provider[]>([]);

  async function fetchProviders() {
    providers.value = await providersApi.list();
  }

  // 拖拽排序：重新赋值数组（保持响应式），并持久化新顺序
  async function reorderProviders(ids: string[]) {
    const map = new Map(providers.value.map((p) => [p.id, p]));
    providers.value = ids.map((id) => map.get(id)).filter((p): p is Provider => !!p);
    try {
      await providersApi.reorder(ids);
    } catch (e: any) {
      // 保存失败时回滚到服务端顺序
      await fetchProviders();
      throw e;
    }
  }

  return {
    providers,
    fetchProviders,
    reorderProviders,
  };
});
