import { defineStore } from 'pinia';
import { ref } from 'vue';
import { dashboardApi } from '../api';
import { wsClient } from '../ws';

export interface DashboardOverview {
  providers: any[];
  total_models: number;
  total_requests: number;
  total_tokens: number;
}

export const useDashboardStore = defineStore('dashboard', () => {
  const overview = ref<DashboardOverview | null>(null);
  const usage = ref<any[]>([]);
  const loading = ref(false);
  const wsConnected = ref(false);

  async function fetchOverview() {
    loading.value = true;
    try {
      overview.value = await dashboardApi.overview();
    } finally {
      loading.value = false;
    }
  }

  async function fetchUsage(from?: number, to?: number) {
    loading.value = true;
    try {
      const result = await dashboardApi.usage({ from, to });
      usage.value = result.usage || [];
    } finally {
      loading.value = false;
    }
  }

  function connectWebSocket() {
    wsClient.on('key_health', (event) => {
      // Update key health in the overview
      if (overview.value) {
        // TODO: find and update the key in the provider's key list
        console.log('Key health update:', event);
      }
    });

    wsClient.on('provider_status', (event) => {
      console.log('Provider status update:', event);
    });

    wsClient.on('balance_update', (event) => {
      console.log('Balance update:', event);
    });

    wsClient.on('request_metrics', (event) => {
      console.log('Request metrics:', event);
    });

    wsClient.connect();
    wsConnected.value = true;
  }

  function disconnectWebSocket() {
    wsClient.disconnect();
    wsConnected.value = false;
  }

  return {
    overview,
    usage,
    loading,
    wsConnected,
    fetchOverview,
    fetchUsage,
    connectWebSocket,
    disconnectWebSocket,
  };
});
