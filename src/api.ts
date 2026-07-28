export const BASE_URL = 'http://localhost:19068';

interface RequestOptions {
  method?: string;
  body?: unknown;
}

// Connection state for offline detection
export const connectionState = {
  isOnline: true,
  lastCheck: 0,
};

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };

  try {
    const res = await fetch(`${BASE_URL}${path}`, {
      method: opts.method || 'GET',
      headers,
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    });

    if (!res.ok) {
      let errorDetail = `${res.status} ${res.statusText}`;
      try {
        const errBody = await res.json();
        if (errBody?.error?.message) {
          errorDetail = errBody.error.message;
        } else if (errBody?.error) {
          errorDetail = typeof errBody.error === 'string' ? errBody.error : JSON.stringify(errBody.error);
        }
      } catch {
        // ignore parse errors
      }
      throw new Error(`API error: ${errorDetail}`);
    }

    connectionState.isOnline = true;
    connectionState.lastCheck = Date.now();
    const text = await res.text();
    return text ? JSON.parse(text) : ({} as T);
  } catch (err: any) {
    // Network error = offline
    if (err.name === 'TypeError' || err.message?.includes('fetch')) {
      connectionState.isOnline = false;
    }
    throw err;
  }
}

// --- Providers ---
export interface Provider {
  id: string;
  name: string;
  kind: 'openai' | 'anthropic';
  base_url: string;
  api_path: string;
  enabled: boolean;
  config: Record<string, any>;
  created_at: number;
  updated_at: number;
}

export const providersApi = {
  list: () => request<Provider[]>('/api/providers'),
  get: (id: string) => request<Provider>(`/api/providers/${id}`),
  create: (data: Partial<Provider>) => request<Provider>('/api/providers', { method: 'POST', body: data }),
  update: (id: string, data: Partial<Provider>) => request<Provider>(`/api/providers/${id}`, { method: 'PUT', body: data }),
  delete: (id: string) => request<{ status: string }>(`/api/providers/${id}`, { method: 'DELETE' }),
};

// --- Service Keys ---
export interface ServiceKey {
  id: string;
  name: string;
  key_masked: string;
  allowed_models: string[];
  total_requests: number;
  total_tokens: number;
  last_used_at: number | null;
  created_at: number;
}

export const serviceKeysApi = {
  list: () => request<ServiceKey[]>('/api/service-keys'),
  create: (data: { name: string }) =>
    request<{ ok: boolean; id: string; key: string }>('/api/service-keys', { method: 'POST', body: data }),
  update: (id: string, data: { name?: string; allowed_models?: string[] }) =>
    request<{ ok: boolean }>(`/api/service-keys/${id}`, { method: 'PUT', body: data }),
  delete: (id: string) => request<{ ok: boolean }>(`/api/service-keys/${id}`, { method: 'DELETE' }),
};

// --- API Keys (provider keys) ---
export interface ApiKey {
  id: string;
  provider_id: string;
  name: string;
  key_masked: string;
  key_plain?: string;
  status: 'green' | 'yellow' | 'red' | 'unknown';
  last_error: string | null;
  last_error_code: number | null;
  last_used_at: number | null;
  created_at: number;
  balance: number | null;
  total_requests: number;
  total_tokens: number;
}

export const keysApi = {
  list: (providerId?: string) => {
    const base = '/api/keys';
    return request<ApiKey[]>(providerId ? `${base}?provider_id=${providerId}` : base);
  },
  create: (providerId: string, data: { name: string; key: string }) =>
    request<ApiKey>(`/api/keys`, { method: 'POST', body: { ...data, provider_id: providerId } }),
  update: (id: string, data: Partial<ApiKey>) =>
    request<ApiKey>(`/api/keys/${id}`, { method: 'PUT', body: data }),
  delete: (providerId: string, keyId: string) =>
    request<{ ok: boolean }>(`/api/keys/${keyId}`, { method: 'DELETE' }),
};

// --- Models ---
export interface Model {
  id: string;
  provider_id: string;
  model_id: string;
  display_name: string;
  tier: 'fable' | 'opus' | 'sonnet' | 'haiku' | 'custom';
  context_window: number;
  max_output_tokens: number;
  cost_per_1k_input: number;
  cost_per_1k_output: number;
  enabled: boolean;
}

export const modelsApi = {
  list: (providerId?: string) => {
    const base = '/api/models';
    return request<Model[]>(providerId ? `${base}?provider_id=${providerId}` : base);
  },
  sync: (providerId: string) =>
    request<{ ok: boolean }>(`/api/models/sync`, { method: 'POST', body: { provider_id: providerId } }),
  create: (data: any) =>
    request<Model>(`/api/models`, { method: 'POST', body: data }),
  update: (modelId: string, data: any) =>
    request<Model>(`/api/models/${modelId}`, { method: 'PUT', body: data }),
  delete: (modelId: string) =>
    request<{ ok: boolean }>(`/api/models/${modelId}`, { method: 'DELETE' }),
};

// --- Dashboard ---
export const dashboardApi = {
  overview: () => request<any>('/api/dashboard/overview'),
  usage: (params: { from?: number; to?: number }) =>
    request<any>(`/api/dashboard/usage?from=${params.from || ''}&to=${params.to || ''}`),
};

// --- Stats ---
export interface StatsRow {
  key_id: string;
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  requests: number;
  day: string;
}
export const statsApi = {
  query: (params: { from: number; to: number; granularity?: 'hour' | 'day'; tz_offset: number }) =>
    request<any>(`/api/stats?from=${params.from}&to=${params.to}&tz_offset=${params.tz_offset}${params.granularity ? `&granularity=${params.granularity}` : ''}`),
};

// --- Public API ---
export const publicApi = {
  health: () => request<any>('/health'),
};

// --- App Settings ---
export const settingsApi = {
  get: () => request<{ websearch_hijack: boolean }>('/api/settings'),
  update: (data: { websearch_hijack?: boolean }) =>
    request<{ status: string }>('/api/settings', { method: 'PUT', body: data }),
};
