interface Window {
  __TAURI__?: {
    shell: {
      open(url: string): Promise<void>;
    };
  };
}
