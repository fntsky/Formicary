import { onBeforeUnmount, ref, watch } from 'vue'
import { requestJson } from './api.js'

export function useGithubAccount(visible, reloadBranches) {
  const account = ref(null)
  const busy = ref(false)
  const error = ref('')
  let timer
  let requestId = 0

  async function update(method) {
    const id = ++requestId
    clearTimeout(timer)
    busy.value = true
    error.value = ''
    try {
      const next = await requestJson('/api/chambers/github/account', { method })
      if (id !== requestId) return
      const wasConnected = account.value?.status === 'connected'
      account.value = next
      if (method === 'GET' || wasConnected !== (next.status === 'connected')) reloadBranches()
      if (next.status === 'pending') {
        timer = setTimeout(() => update('PATCH'), next.retry_after * 1000)
      } else if (next.status === 'connected') {
        timer = setTimeout(() => update('GET'), next.expires_in * 1000)
      } else if (method === 'PATCH' && next.status === 'disconnected') {
        error.value = 'GitHub 授权码已过期，请重新登录。'
      }
    } catch (requestError) {
      if (id !== requestId) return
      error.value = requestError.message
      if (method === 'GET') account.value = null
      if (method === 'PATCH') account.value = { status: 'disconnected' }
    } finally {
      if (id === requestId) busy.value = false
    }
  }

  function stop() {
    requestId++
    clearTimeout(timer)
    account.value = null
    busy.value = false
    error.value = ''
  }

  watch(visible, (isVisible) => {
    if (isVisible) update('GET')
    else stop()
  })
  onBeforeUnmount(stop)

  return { account, busy, error, update }
}
