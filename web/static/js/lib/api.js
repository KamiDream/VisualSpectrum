/**
 * 后端 REST 封装。
 *
 * 所有接口都返回 `{ok: true, ...}` 或 `{ok: false, error: "中文说明"}`；
 * 这里把 `ok:false` 统一变成 `throw new Error(error)`，上层只管 try/catch。
 */

async function readJSON(response) {
  let payload = null
  try {
    payload = await response.json()
  } catch (error) {
    throw new Error(`后端返回的不是 JSON（HTTP ${response.status}）`)
  }
  if (!payload || payload.ok !== true) {
    throw new Error(payload?.error || `请求失败（HTTP ${response.status}）`)
  }
  return payload
}

export async function getJSON(path) {
  return readJSON(await fetch(path, { headers: { Accept: 'application/json' } }))
}

export async function sendJSON(method, path, body) {
  return readJSON(await fetch(path, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  }))
}

export const api = {
  meta: () => getJSON('/api/meta'),
  config: () => getJSON('/api/config'),
  spectrum: () => getJSON('/api/spectrum'),
  devices: () => getJSON('/api/audio/devices'),
  save: (config) => sendJSON('PUT', '/api/config', config),
  validate: (config) => sendJSON('POST', '/api/config/validate', config),
  reset: () => sendJSON('POST', '/api/config/reset'),
  reload: () => sendJSON('POST', '/api/reload', {}),
  overlay: (visible) => sendJSON('POST', '/api/overlay',
    visible === undefined ? {} : { visible }),
}
