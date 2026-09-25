// Cross-browser namespace with promise-based APIs (Firefox `browser`, Chrome `chrome`).
export const api = globalThis.browser ?? globalThis.chrome

export async function loadConfig() {
  const stored = await api.storage.local.get({ server: 'http://127.0.0.1:8710', token: '', interceptDownloads: true })
  return { server: stored.server, token: stored.token, interceptDownloads: stored.interceptDownloads !== false }
}

export async function saveConfig(config) {
  await api.storage.local.set({
    server: config.server,
    token: config.token,
    interceptDownloads: config.interceptDownloads !== false
  })
}

export function message(key, substitutions) {
  return api.i18n.getMessage(key, substitutions) || key
}
