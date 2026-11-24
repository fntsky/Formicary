export async function requestJson(endpoint, options = {}) {
  const response = await fetch(endpoint, {
    credentials: 'same-origin',
    ...options,
  })
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(data.error || '请求失败，请稍后重试。')
  }
  return data
}

export function postJson(endpoint, payload) {
  return requestJson(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
}
