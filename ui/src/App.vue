<script setup>
import { computed, onMounted, ref, watch, onBeforeUnmount } from 'vue'

const username = ref('')
const password = ref('')
const currentUser = ref('')
const newUsername = ref('')
const renamePassword = ref('')
const currentPassword = ref('')
const newPassword = ref('')
const error = ref('')
const notice = ref('')
const busy = ref(false)

const activePage = ref('chambers')
const chamberTab = ref('mounts')
const chambers = ref([])
const selectedChamber = ref('')
const chamberDetails = ref(null)
const chamberError = ref('')
const chamberNotice = ref('')
const loadingChambers = ref(false)
const loadingChamber = ref(false)
const refreshing = ref(false)
const busyJob = ref(null)
let chamberRequestId = 0

const keys = ref([])
const loadingKeys = ref(false)
const keysError = ref('')
const keysNotice = ref('')
const createKeyDialog = ref(null)
const createKeyButton = ref(null)
const createKeyName = ref('')
const createKeyPassword = ref('')
const createKeyError = ref('')
const creatingKey = ref(false)
const createdKey = ref(null)
const createdKeyDialog = ref(null)
const copyKeyStatus = ref('')
const revokeKeyDialog = ref(null)
const revokeKeyTarget = ref(null)
const revokeKeyError = ref('')
const revokingKey = ref(false)

const totalJobs = computed(() => chambers.value.reduce((total, chamber) => total + chamber.job_count, 0))
const enabledJobs = computed(() => chambers.value.reduce((total, chamber) => total + chamber.enabled_job_count, 0))
const totalMounts = computed(() => chambers.value.reduce(
  (total, chamber) => total + chamber.static_web_count + chamber.proxy_http_count,
  0,
))

async function readResponse(response) {
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(data.error || '请求失败，请稍后重试。')
  }
  return data
}

async function requestJson(endpoint, options = {}) {
  const response = await fetch(endpoint, {
    credentials: 'same-origin',
    ...options,
  })
  return readResponse(response)
}

async function postJson(endpoint, payload) {
  return requestJson(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
}

const proxyLogsChamber = ref('')
const proxyLogs = ref([])
const proxyLogsRoute = ref('')
const proxyLogsLoading = ref(false)
const proxyLogsError = ref('')
let proxyLogsRequestId = 0

async function openProxyLogs(mount, index) {
  const requestId = ++proxyLogsRequestId
  proxyLogsChamber.value = selectedChamber.value
  proxyLogsRoute.value = mount.route
  proxyLogs.value = []
  proxyLogsError.value = ''
  proxyLogsLoading.value = true
  activePage.value = 'proxy-logs'
  try {
    const data = await requestJson(`/api/chambers/${encodeURIComponent(selectedChamber.value)}/proxies/${index}/logs`, { cache: 'no-store' })
    if (requestId !== proxyLogsRequestId) return
    const decoders = { stdout: new TextDecoder(), stderr: new TextDecoder() }
    const entries = data.entries.map((entry) => ({
      ...entry,
      text: decoders[entry.stream].decode(new Uint8Array(entry.bytes), { stream: true }),
      time: new Date(entry.timestamp).toLocaleString('zh-CN', { hour12: false }),
    }))
    for (const stream of ['stdout', 'stderr']) {
      const tail = decoders[stream].decode()
      const last = entries.findLast((entry) => entry.stream === stream)
      if (last) last.text += tail
    }
    proxyLogs.value = entries
  } catch (error) {
    if (requestId === proxyLogsRequestId) proxyLogsError.value = error.message
  } finally {
    if (requestId === proxyLogsRequestId) proxyLogsLoading.value = false
  }
}

function closeProxyLogs() {
  proxyLogsRequestId++
  activePage.value = 'chambers'
  chamberTab.value = 'mounts'
}

const importDialog = ref(null)
const importButton = ref(null)
const importStage = ref('')

function openImportDialog() {
  githubError.value = ''
  githubMessage.value = ''
  importStage.value = ''
  importDialog.value.showModal()
}

function closeImportDialog() {
  if (downloadingGithub.value) return
  importDialog.value.close()
  importButton.value?.focus()
}

const github = ref({ repository: '', branch: '', depth: 1 })
const downloadingGithub = ref(false)
const githubMessage = ref('')
const githubError = ref('')

const githubBranches = ref([])
const githubDefaultBranch = ref('')
const loadingGithubBranches = ref(false)
const githubBranchError = ref('')
let branchRequestId = 0
let branchTimer

watch(() => github.value.repository, () => {
  clearTimeout(branchTimer)
  branchRequestId++
  githubBranches.value = []
  github.value.branch = ''
  githubDefaultBranch.value = ''
  githubBranchError.value = ''
  loadingGithubBranches.value = !!github.value.repository.trim()
  if (loadingGithubBranches.value) branchTimer = setTimeout(fetchGithubBranches, 600)
})
onBeforeUnmount(() => { clearTimeout(branchTimer); branchRequestId++ })

async function fetchGithubBranches() {
  clearTimeout(branchTimer)
  const requestId = ++branchRequestId
  loadingGithubBranches.value = true
  githubBranchError.value = ''
  try {
    const data = await postJson('/api/chambers/github/branches', { repository: github.value.repository })
    if (requestId !== branchRequestId) return
    githubBranches.value = data.branches
    githubDefaultBranch.value = data.default_branch || ''
    github.value.branch = data.branches.includes(data.default_branch) ? data.default_branch : data.branches[0]
  } catch (error) {
    if (requestId === branchRequestId) githubBranchError.value = error.message
  } finally {
    if (requestId === branchRequestId) loadingGithubBranches.value = false
  }
}

async function downloadGithub() {
  if (downloadingGithub.value || githubMessage.value || loadingGithubBranches.value || !github.value.branch) return
  downloadingGithub.value = true
  githubMessage.value = ''
  githubError.value = ''
  importStage.value = '正在下载仓库并加载 Chamber…'
  try {
    const data = await postJson('/api/chambers/import', {
      ...github.value,
      depth: github.value.depth === '' ? null : Number(github.value.depth),
    })
    importStage.value = '正在刷新 Chamber 列表…'
    selectedChamber.value = data.name
    await loadChambers()
    importStage.value = '导入完成'
    githubMessage.value = `Chamber ${data.name} 已导入并加载。`
  } catch (error) {
    importStage.value = '导入失败'
    githubError.value = error.message
  } finally {
    downloadingGithub.value = false
  }
}

const lifecycleBusy = ref(false)
const upgradeRepository = ref('')
const upgradeBranch = ref('main')
async function chamberAction(action, method = 'POST') {
  const name = selectedChamber.value
  if (method === 'DELETE' && !window.confirm(action ? '确定删除这个旧版本？' : `确定删除 ${name}？所有版本、数据库和上传文件都将永久删除。`)) return
  lifecycleBusy.value = true
  chamberError.value = ''
  try {
    await requestJson(`/api/chambers/${encodeURIComponent(name)}${action ? '/' + action : ''}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      ...(action === 'upgrade' ? { body: JSON.stringify({ repository: upgradeRepository.value, branch: upgradeBranch.value, depth: 1 }) } : {}),
    })
    await loadChambers()
    chamberNotice.value = '操作完成。'
  } catch (error) {
    await loadChambers()
    chamberError.value = error.message
  } finally { lifecycleBusy.value = false }
}

async function loadChamber(name) {
  const requestId = ++chamberRequestId
  selectedChamber.value = name
  chamberError.value = ''
  chamberNotice.value = ''
  loadingChamber.value = true
  try {
    const data = await requestJson(`/api/chambers/${encodeURIComponent(name)}`)
    if (requestId === chamberRequestId) {
      chamberDetails.value = data.chamber
    }
  } catch (requestError) {
    if (requestId === chamberRequestId) {
      chamberDetails.value = null
      chamberError.value = requestError.message
    }
  } finally {
    if (requestId === chamberRequestId) {
      loadingChamber.value = false
    }
  }
}

async function loadChambers() {
  loadingChambers.value = true
  chamberError.value = ''
  try {
    const data = await requestJson('/api/chambers')
    chambers.value = data.chambers || []
    if (!chambers.value.some((chamber) => chamber.name === selectedChamber.value)) {
      selectedChamber.value = chambers.value[0]?.name || ''
    }
    if (selectedChamber.value) {
      await loadChamber(selectedChamber.value)
    } else {
      chamberDetails.value = null
    }
  } catch (requestError) {
    chamberError.value = requestError.message
  } finally {
    loadingChambers.value = false
  }
}

async function refreshChambers() {
  refreshing.value = true
  await loadChambers()
  refreshing.value = false
}

async function toggleJob(job) {
  busyJob.value = job.id
  chamberError.value = ''
  chamberNotice.value = ''
  try {
    const action = job.enabled ? 'pause' : 'start'
    const data = await postJson(`/api/jobs/${job.id}/${action}`)
    if (chamberDetails.value?.name === data.job.chamber) {
      chamberDetails.value.jobs = chamberDetails.value.jobs.map((item) => (
        item.id === job.id ? data.job : item
      ))
      chamberDetails.value.enabled_job_count = chamberDetails.value.jobs.filter((item) => item.enabled).length
    }
    const summary = chambers.value.find((item) => item.name === data.job.chamber)
    if (summary) {
      summary.enabled_job_count += data.job.enabled ? 1 : -1
    }
    if (selectedChamber.value === data.job.chamber) {
      chamberNotice.value = data.job.enabled ? '任务已启动。' : '任务已暂停。'
    }
  } catch (requestError) {
    if (selectedChamber.value === job.chamber) {
      chamberError.value = requestError.message
    }
  } finally {
    busyJob.value = null
  }
}

async function login() {
  error.value = ''
  notice.value = ''
  busy.value = true
  try {
    const data = await postJson('/api/login', {
      username: username.value,
      password: password.value,
    })
    currentUser.value = data.username
    newUsername.value = data.username
    password.value = ''
    activePage.value = 'chambers'
    await loadChambers()
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

async function changeUsername() {
  error.value = ''
  notice.value = ''
  busy.value = true
  try {
    const data = await postJson('/api/admin/username', {
      username: newUsername.value,
      current_password: renamePassword.value,
    })
    currentUser.value = data.username
    username.value = data.username
    renamePassword.value = ''
    notice.value = '管理员用户名已更新。'
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

async function changePassword() {
  error.value = ''
  notice.value = ''
  busy.value = true
  try {
    await postJson('/api/admin/password', {
      current_password: currentPassword.value,
      new_password: newPassword.value,
    })
    currentUser.value = ''
    password.value = ''
    currentPassword.value = ''
    newPassword.value = ''
    notice.value = '密码已更新，请使用新密码重新登录。'
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

async function logout() {
  error.value = ''
  notice.value = ''
  busy.value = true
  try {
    await requestJson('/api/logout', { method: 'POST' })
    currentUser.value = ''
    password.value = ''
    chambers.value = []
    chamberDetails.value = null
    selectedChamber.value = ''
    keys.value = []
    createdKey.value = null
    createKeyPassword.value = ''
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

function formatTimeout(timeoutMs) {
  if (timeoutMs >= 3_600_000 && timeoutMs % 3_600_000 === 0) {
    return `${timeoutMs / 3_600_000} 小时`
  }
  if (timeoutMs >= 60_000 && timeoutMs % 60_000 === 0) {
    return `${timeoutMs / 60_000} 分钟`
  }
  return `${Math.round(timeoutMs / 1000)} 秒`
}

async function loadKeys() {
  loadingKeys.value = true
  keysError.value = ''
  try {
    const data = await requestJson('/api/keys', { cache: 'no-store' })
    keys.value = data.keys || []
  } catch (requestError) {
    keysError.value = requestError.message
  } finally {
    loadingKeys.value = false
  }
}

function openCreateKeyDialog() {
  createKeyError.value = ''
  keysNotice.value = ''
  createKeyName.value = ''
  createKeyPassword.value = ''
  createKeyDialog.value?.showModal()
}

function closeCreateKeyDialog() {
  if (creatingKey.value) return
  createKeyPassword.value = ''
  createKeyDialog.value?.close()
  createKeyButton.value?.focus()
}

async function createKey() {
  if (creatingKey.value) return
  creatingKey.value = true
  createKeyError.value = ''
  keysNotice.value = ''
  try {
    const data = await postJson('/api/keys', {
      name: createKeyName.value,
      current_password: createKeyPassword.value,
    })
    createKeyPassword.value = ''
    createKeyDialog.value?.close()
    createdKey.value = data
    copyKeyStatus.value = ''
    createdKeyDialog.value?.showModal()
    await loadKeys()
  } catch (requestError) {
    createKeyError.value = requestError.message
  } finally {
    creatingKey.value = false
  }
}

async function copyCreatedKey() {
  if (!createdKey.value) return
  try {
    await navigator.clipboard.writeText(createdKey.value.token)
    copyKeyStatus.value = '已复制到剪贴板。'
  } catch {
    copyKeyStatus.value = '复制失败，请手动选择密钥文本。'
  }
}

function closeCreatedKey() {
  createdKey.value = null
  copyKeyStatus.value = ''
  createdKeyDialog.value?.close()
}

function openRevokeKeyDialog(key) {
  revokeKeyError.value = ''
  keysNotice.value = ''
  revokeKeyTarget.value = key
  revokeKeyDialog.value?.showModal()
}

function closeRevokeKeyDialog() {
  if (revokingKey.value) return
  revokeKeyTarget.value = null
  revokeKeyDialog.value?.close()
}

async function revokeKey() {
  if (!revokeKeyTarget.value || revokingKey.value) return
  revokingKey.value = true
  revokeKeyError.value = ''
  keysNotice.value = ''
  const key = revokeKeyTarget.value
  try {
    await requestJson(`/api/keys/${key.id}`, { method: 'DELETE' })
    revokeKeyDialog.value?.close()
    revokeKeyTarget.value = null
    keysNotice.value = `密钥 ${key.name} 已撤销。`
    await loadKeys()
  } catch (requestError) {
    revokeKeyError.value = requestError.message
  } finally {
    revokingKey.value = false
  }
}

function formatDateTime(value) {
  if (!value) return '—'
  const date = new Date(value)
  return Number.isNaN(date.getTime())
    ? '—'
    : date.toLocaleString('zh-CN', { hour12: false })
}

function formatLastUsed(value) {
  return value ? formatDateTime(value) : '从未使用'
}

function maskKeyId(keyId) {
  return keyId ? `fmk_${keyId.slice(0, 8)}…` : '—'
}

function keyStatusLabel(status) {
  return status === 'revoked' ? '已撤销' : '有效'
}

watch(activePage, (page) => {
  if (page === 'keys' && currentUser.value) loadKeys()
})

onMounted(async () => {
  try {
    const data = await requestJson('/api/me')
    currentUser.value = data.username
    username.value = data.username
    newUsername.value = data.username
    await loadChambers()
  } catch {
    // An unauthenticated session is the normal initial state.
  }
})
</script>

<template>
  <div class="page-shell" :class="{ 'dashboard-page': currentUser }">
    <header class="topbar">
      <a class="brand" href="/ui/" aria-label="Formicary 控制面板">
        <span class="brand-mark" aria-hidden="true">F</span>
        <span>Formicary</span>
      </a>
      <template v-if="currentUser">
        <nav class="topbar-nav" aria-label="管理页面">
          <button :class="{ selected: activePage === 'chambers' || activePage === 'proxy-logs' }" @click="activePage = 'chambers'">服务管理</button>
          <button :class="{ selected: activePage === 'keys' }" @click="activePage = 'keys'">API 密钥</button>
          <button :class="{ selected: activePage === 'account' }" @click="activePage = 'account'">账户设置</button>
        </nav>
        <div class="topbar-user"><span class="user-avatar">{{ currentUser.slice(0, 1).toUpperCase() }}</span>{{ currentUser }}</div>
      </template>
      <span v-else class="topbar-note">管理员控制面板</span>
    </header>

    <main v-if="!currentUser" class="main-content">
      <section class="welcome-panel" aria-label="控制面板介绍">
        <div class="welcome-kicker"><span></span> FORMICARY CONTROL PANEL</div>
        <h1>管理你的<br />Formicary 服务。</h1>
        <p>查看 Chamber、管理定时任务和维护管理员账户。</p>
        <div class="welcome-decoration" aria-hidden="true">
          <div class="decoration-card decoration-card-back"></div>
          <div class="decoration-card decoration-card-front">
            <span class="decoration-dot"></span>
            <span class="decoration-line line-long"></span>
            <span class="decoration-line line-short"></span>
            <span class="decoration-block"></span>
          </div>
        </div>
      </section>

      <section class="auth-card" aria-labelledby="form-title">
        <p class="eyebrow">ADMIN ACCESS</p>
        <h2 id="form-title">管理员登录</h2>
        <p class="card-description">使用 Formicary 管理员账户继续。</p>

        <p v-if="notice" class="notice-message" role="status">{{ notice }}</p>
        <form class="auth-form" @submit.prevent="login">
          <label for="username">用户名</label>
          <input
            id="username"
            v-model.trim="username"
            name="username"
            type="text"
            autocomplete="username"
            minlength="3"
            maxlength="32"
            required
          />

          <label for="password">密码</label>
          <input
            id="password"
            v-model="password"
            name="password"
            type="password"
            autocomplete="current-password"
            maxlength="128"
            required
          />

          <p v-if="error" class="error-message" role="alert">{{ error }}</p>

          <button class="primary-button" type="submit" :disabled="busy">
            {{ busy ? '请稍候…' : '登录控制面板' }}
            <span aria-hidden="true">→</span>
          </button>
        </form>
        <div class="card-footer"><span class="lock-icon" aria-hidden="true">⌑</span> 仅管理员可访问</div>
      </section>
    </main>

    <main v-else-if="activePage === 'chambers'" class="dashboard-main">
      <div class="dashboard-heading">
        <div>
          <h1>Chamber 管理</h1>
        </div>
        <div class="dashboard-actions">
        <button ref="importButton" class="primary-button import-trigger" type="button" @click="openImportDialog">＋ 从 GitHub 导入</button>
        <button class="refresh-button" type="button" :disabled="refreshing" @click="refreshChambers">
          <span aria-hidden="true">↻</span>{{ refreshing ? '正在刷新' : '刷新数据' }}
        </button>
        </div>
      </div>

      <p v-if="chamberError" class="dashboard-message error-message" role="alert">{{ chamberError }}</p>
      <p v-if="chamberNotice" class="dashboard-message notice-message" role="status">{{ chamberNotice }}</p>

      <dialog ref="importDialog" class="import-dialog" aria-labelledby="import-title" aria-describedby="import-description" @cancel.prevent="closeImportDialog" @click="($event.target === importDialog) && closeImportDialog()">
        <div class="import-dialog-heading">
          <div><p class="eyebrow">IMPORT CHAMBER</p><h2 id="import-title">从 GitHub 导入</h2></div>
          <button class="dialog-close" type="button" aria-label="关闭导入弹窗" :disabled="downloadingGithub" @click="closeImportDialog">×</button>
        </div>
        <p id="import-description">仓库将自动下载到 Chamber 目录，以 chamber.toml 中的名称创建子目录。导入后会启用配置的任务和服务。</p>
        <form class="github-download" :aria-busy="downloadingGithub" @submit.prevent="downloadGithub">
          <fieldset :disabled="downloadingGithub || !!githubMessage">
            <label class="repository-field">GitHub 仓库<input v-model="github.repository" autofocus required placeholder="owner/repo 或 https://github.com/owner/repo" /></label>
            <label>目标分支
              <select v-model="github.branch" required :disabled="loadingGithubBranches || !githubBranches.length" aria-describedby="branch-status">
                <option disabled value="">{{ loadingGithubBranches ? '正在获取分支…' : '请先填写仓库地址' }}</option>
                <option v-for="branch in githubBranches" :key="branch" :value="branch">{{ branch }}{{ branch === githubDefaultBranch ? '（默认）' : '' }}</option>
              </select>
            </label>
            <div id="branch-status" class="repository-field" aria-live="polite">
              <span v-if="loadingGithubBranches">正在自动获取仓库分支…</span>
              <template v-else-if="githubBranchError"><span class="error-message">{{ githubBranchError }}</span> <button type="button" class="inline-link" @click="fetchGithubBranches">重新获取</button></template>
              <span v-else-if="githubBranches.length">已获取 {{ githubBranches.length }} 个分支，可切换目标分支。</span>
            </div>
            <label>下载深度<input v-model="github.depth" type="number" min="1" max="4294967295" step="1" placeholder="留空下载完整历史" /></label>
          </fieldset>
          <div v-if="importStage" class="import-progress" aria-live="polite">
            <div class="import-progress-label"><span>{{ importStage }}</span><span v-if="githubMessage">100%</span></div>
            <progress v-if="downloadingGithub" aria-label="Chamber 导入进行中"></progress>
            <progress v-else-if="githubMessage" aria-label="Chamber 导入完成" :value="100" :max="100"></progress>
            <p v-if="downloadingGithub" class="import-progress-help">请稍候，下载及加载期间请保持此窗口打开。</p>
          </div>
          <p v-if="githubError" class="error-message" role="alert">{{ githubError }}</p>
          <p v-if="githubMessage" class="notice-message" role="status">{{ githubMessage }}</p>
          <div class="import-dialog-actions">
            <button class="refresh-button" type="button" :disabled="downloadingGithub" @click="closeImportDialog">{{ githubMessage ? '完成' : '取消' }}</button>
            <button v-if="!githubMessage" class="primary-button" type="submit" :disabled="downloadingGithub || loadingGithubBranches || !github.branch">{{ downloadingGithub ? '正在导入…' : githubError ? '重试导入' : '导入 Chamber' }}</button>
          </div>
        </form>
      </dialog>

      <div class="service-summary" aria-label="服务统计">
        <span><strong>{{ chambers.length }}</strong> Chambers</span>
        <span><strong>{{ totalJobs }}</strong> 定时任务 <small>{{ enabledJobs }} 已启用</small></span>
        <span><strong>{{ totalMounts }}</strong> 服务挂载</span>
      </div>

      <section class="chamber-layout">
        <aside class="chamber-list-panel" aria-label="Chamber 列表">
          <div class="panel-heading">
            <h2>Chambers</h2>
            <span class="count-pill">{{ chambers.length }}</span>
          </div>

          <div v-if="loadingChambers && !chambers.length" class="panel-empty">正在读取 Chamber…</div>
          <div v-else-if="!chambers.length" class="panel-empty">
            <span class="empty-mark" aria-hidden="true">◈</span>
            <strong>还没有 Chamber</strong>
          </div>
          <div v-else class="chamber-list">
            <button
              v-for="item in chambers"
              :key="item.name"
              class="chamber-list-item"
              :class="{ active: selectedChamber === item.name }"
              type="button"
              :disabled="lifecycleBusy"
              @click="loadChamber(item.name)"
            >
              <span class="chamber-status-dot" aria-hidden="true"></span>
              <span class="chamber-list-copy">
                <strong>{{ item.name }}</strong>
                <span>{{ item.job_count }} 个任务 · {{ item.static_web_count + item.proxy_http_count }} 个挂载</span>
              </span>
              <span class="chamber-chevron" aria-hidden="true">›</span>
            </button>
          </div>

        </aside>

        <section class="chamber-detail-panel" aria-live="polite">
          <div v-if="loadingChamber" class="detail-empty">正在读取 Chamber 详情…</div>
          <div v-else-if="!chamberDetails" class="detail-empty">
            <span class="empty-mark" aria-hidden="true">◈</span>
            <strong>{{ chambers.length ? '选择一个 Chamber' : '等待 Chamber 配置' }}</strong>
          </div>
          <template v-else>
            <div class="detail-heading">
              <div class="detail-title-group">
                <span class="detail-avatar" aria-hidden="true">{{ chamberDetails.name.slice(0, 1).toUpperCase() }}</span>
                <div>
                  <div class="detail-name-line"><h2>{{ chamberDetails.name }}</h2><span class="loaded-badge"><i></i>{{ chamberDetails.stopped ? '已关停' : '运行中' }}</span></div>
                  <p class="path-label"><span aria-hidden="true">⌂</span>{{ chamberDetails.directory }}</p>
                </div>
              </div>
            </div>

            <section class="empty-card" aria-label="版本和生命周期管理">
              <p>当前版本：{{ chamberDetails.current_commit }}</p>
              <button class="refresh-button" :disabled="lifecycleBusy" @click="chamberAction(chamberDetails.stopped ? 'start' : 'stop')">{{ chamberDetails.stopped ? '启动 Chamber' : '关停 Chamber' }}</button>
              <button class="refresh-button" :disabled="lifecycleBusy" @click="chamberAction('', 'DELETE')">删除整个 Chamber</button>
              <form @submit.prevent="chamberAction('upgrade')">
                <label>升级仓库 <input v-model="upgradeRepository" placeholder="owner/repo" required :disabled="lifecycleBusy" /></label>
                <label>分支 <input v-model="upgradeBranch" required :disabled="lifecycleBusy" /></label>
                <button class="primary-button" :disabled="lifecycleBusy">{{ lifecycleBusy ? '正在处理，请稍候…' : '准备、校验并升级' }}</button>
              </form>
              <p>升级失败自动回滚代码。共享数据库迁移需要兼容旧版本。</p>
              <ul><li v-for="commit in chamberDetails.releases" :key="commit">{{ commit }}
                <button v-if="commit !== chamberDetails.current_commit" :disabled="lifecycleBusy" @click="chamberAction('releases/' + encodeURIComponent(commit), 'DELETE')">删除旧版本</button>
              </li></ul>
            </section>

            <nav class="detail-tabs" aria-label="Chamber 详情">
              <button :class="{ selected: chamberTab === 'jobs' }" @click="chamberTab = 'jobs'">定时任务 <span>{{ chamberDetails.job_count }}</span></button>
              <button :class="{ selected: chamberTab === 'mounts' }" @click="chamberTab = 'mounts'">服务挂载 <span>{{ chamberDetails.static_web.length + chamberDetails.proxy_http.length }}</span></button>
            </nav>

            <div v-if="chamberTab === 'jobs'" class="detail-body">
              <div class="tab-content-heading"><div><h3>定时任务</h3></div><span class="count-pill">{{ chamberDetails.jobs.length }} 项</span></div>
              <div v-if="!chamberDetails.jobs.length" class="empty-card">这个 Chamber 还没有定时任务。</div>
              <div v-else class="table-wrap">
                <table class="jobs-table">
                  <thead><tr><th>任务</th><th>计划</th><th>执行目录</th><th>超时</th><th>状态</th><th></th></tr></thead>
                  <tbody>
                    <tr v-for="job in chamberDetails.jobs" :key="job.id">
                      <td><strong>{{ job.name }}</strong><code>{{ job.command }}</code></td>
                      <td><span class="cron-pill">{{ job.cron }}</span></td>
                      <td class="directory-cell" :title="job.working_directory">{{ job.working_directory }}</td>
                      <td class="muted-cell">{{ formatTimeout(job.timeout_ms) }}</td>
                      <td><span class="job-status" :class="{ paused: !job.enabled }"><i></i>{{ job.enabled ? '已启用' : '已暂停' }}</span></td>
                      <td><button class="job-action" :class="{ resume: !job.enabled }" :disabled="busyJob === job.id" @click="toggleJob(job)">{{ busyJob === job.id ? '处理中…' : job.enabled ? '暂停' : '启动' }}</button></td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>

            <div v-else class="detail-body">
              <div class="tab-content-heading"><div><h3>服务挂载</h3></div><span class="count-pill">{{ chamberDetails.static_web.length + chamberDetails.proxy_http.length }} 项</span></div>
              <div v-if="!chamberDetails.static_web.length && !chamberDetails.proxy_http.length" class="empty-card">暂无服务挂载。</div>
              <div v-else class="mount-groups">
                <section v-if="chamberDetails.static_web.length" class="mount-group">
                  <div class="mount-group-heading"><span class="mount-type-icon static-mount-icon">▤</span><div><h4>静态目录</h4></div><span class="count-pill">{{ chamberDetails.static_web.length }}</span></div>
                  <div v-if="!chamberDetails.static_web.length" class="small-empty">没有静态目录挂载。</div>
                  <div v-for="mount in chamberDetails.static_web" :key="mount.route" class="mount-card">
                    <div class="mount-route-line"><span class="route-method get-method">GET</span><strong>{{ mount.route }}</strong></div>
                    <p><span>目录</span><code>{{ mount.directory }}</code></p>
                  </div>
                </section>
                <section v-if="chamberDetails.proxy_http.length" class="mount-group">
                  <div class="mount-group-heading"><span class="mount-type-icon proxy-mount-icon">⇄</span><div><h4>HTTP 代理</h4></div><span class="count-pill">{{ chamberDetails.proxy_http.length }}</span></div>
                  <div v-if="!chamberDetails.proxy_http.length" class="small-empty">没有 HTTP 代理挂载。</div>
                  <div v-for="(mount, index) in chamberDetails.proxy_http" :key="mount.route" class="mount-card">
                    <div class="mount-route-line"><span class="route-method proxy-method">PROXY</span><strong>{{ mount.route }}</strong><button type="button" class="inline-link mount-logs-link" @click="openProxyLogs(mount, index)">查看日志</button></div>
                    <p><span>上游</span><code>{{ mount.upstream }}</code></p>
                    <p><span>工作目录</span><code>{{ mount.working_directory }}</code></p>
                  </div>
                </section>
              </div>
            </div>
          </template>
        </section>
      </section>
    </main>

    <main v-else-if="activePage === 'proxy-logs'" class="dashboard-main proxy-logs-page">
      <button type="button" class="inline-link proxy-logs-back" @click="closeProxyLogs">← 返回服务挂载</button>
      <div class="dashboard-heading">
        <div>
          <h1>代理日志</h1>
          <p class="dashboard-subtitle">{{ proxyLogsChamber }} <span aria-hidden="true">/</span> {{ proxyLogsRoute }}</p>
        </div>
        <span v-if="!proxyLogsLoading && !proxyLogsError" class="count-pill">{{ proxyLogs.length }} 条记录</span>
      </div>
      <p v-if="proxyLogsLoading" class="panel-empty" role="status">正在读取日志…</p>
      <p v-else-if="proxyLogsError" class="dashboard-message error-message" role="alert">{{ proxyLogsError }}</p>
      <p v-else-if="!proxyLogs.length" class="panel-empty">暂无日志输出。</p>
      <div v-else class="proxy-log-output" tabindex="0" aria-label="代理日志内容">
        <div v-for="(entry, index) in proxyLogs" :key="index" class="proxy-log-entry" :class="{ 'proxy-log-error': entry.stream === 'stderr' }">
          <span class="proxy-log-stream">{{ entry.stream }}</span>
          <pre>{{ entry.text }}</pre>
          <time class="proxy-log-time" :datetime="entry.timestamp">{{ entry.time }}</time>
        </div>
      </div>
    </main>

    <main v-else-if="activePage === 'keys'" class="dashboard-main keys-page">
      <div class="dashboard-heading">
        <div>
          <h1>API 密钥</h1>
          <p class="dashboard-subtitle">用于 Chamber 代理服务的密钥认证。密钥只显示一次，请妥善保存。</p>
        </div>
        <button ref="createKeyButton" class="primary-button import-trigger" type="button" @click="openCreateKeyDialog">＋ 创建密钥</button>
      </div>

      <p class="key-warning">
        所有密钥都会被所有 Chamber 代理识别为有效身份，且永不过期。Chamber 通过
        <code>X-Formicary-Auth</code> 判断请求是否携带有效密钥，并自行决定哪些接口需要密钥。
      </p>

      <p v-if="keysError" class="dashboard-message error-message" role="alert">{{ keysError }}</p>
      <p v-if="keysNotice" class="dashboard-message notice-message" role="status">{{ keysNotice }}</p>

      <p v-if="loadingKeys" class="panel-empty" role="status">正在读取密钥…</p>
      <p v-else-if="!keys.length" class="panel-empty">还没有 API 密钥。创建后可用于访问需要密钥的 Chamber 代理接口。</p>
      <div v-else class="table-wrap">
        <table class="jobs-table keys-table">
          <thead>
            <tr><th>名称</th><th>密钥</th><th>创建时间</th><th>最近使用</th><th>状态</th><th></th></tr>
          </thead>
          <tbody>
            <tr v-for="key in keys" :key="key.id">
              <td><strong>{{ key.name }}</strong><small>#{{ key.id }}</small></td>
              <td><code class="key-prefix">{{ maskKeyId(key.key_id) }}</code></td>
              <td class="muted-cell">{{ formatDateTime(key.created_at) }}</td>
              <td class="muted-cell">{{ formatLastUsed(key.last_used_at) }}</td>
              <td><span class="job-status" :class="{ paused: key.status === 'revoked' }"><i></i>{{ keyStatusLabel(key.status) }}</span></td>
              <td>
                <button v-if="key.status !== 'revoked'" class="job-action" type="button" @click="openRevokeKeyDialog(key)">撤销</button>
                <span v-else class="muted-cell">—</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <dialog ref="createKeyDialog" class="import-dialog key-dialog" aria-labelledby="create-key-title" @cancel.prevent="closeCreateKeyDialog" @click="($event.target === createKeyDialog) && closeCreateKeyDialog()">
        <div class="import-dialog-heading">
          <div><p class="eyebrow">CREATE API KEY</p><h2 id="create-key-title">创建密钥</h2></div>
          <button class="dialog-close" type="button" aria-label="关闭创建密钥弹窗" :disabled="creatingKey" @click="closeCreateKeyDialog">×</button>
        </div>
        <p>密钥用于 Chamber 代理服务的认证。没有作用域和过期时间，任何有效密钥都可以被所有 Chamber 使用。</p>
        <form class="auth-form" @submit.prevent="createKey">
          <label for="key-name">名称</label>
          <input id="key-name" v-model.trim="createKeyName" name="key-name" type="text" maxlength="64" required autofocus placeholder="例如：blog-ci" />
          <label for="key-password">当前密码</label>
          <input id="key-password" v-model="createKeyPassword" name="key-password" type="password" autocomplete="current-password" maxlength="128" required />
          <p v-if="createKeyError" class="error-message" role="alert">{{ createKeyError }}</p>
          <div class="import-dialog-actions">
            <button class="refresh-button" type="button" :disabled="creatingKey" @click="closeCreateKeyDialog">取消</button>
            <button class="primary-button" type="submit" :disabled="creatingKey">{{ creatingKey ? '正在创建…' : '创建密钥' }}</button>
          </div>
        </form>
      </dialog>

      <dialog ref="createdKeyDialog" class="import-dialog key-dialog" aria-labelledby="created-key-title" @cancel.prevent>
        <div class="import-dialog-heading">
          <div><p class="eyebrow">KEY CREATED</p><h2 id="created-key-title">密钥已创建</h2></div>
        </div>
        <p>这是唯一一次显示完整密钥。关闭后无法再次查看，请立即保存。</p>
        <p v-if="createdKey" class="key-secret">{{ createdKey.token }}</p>
        <p v-if="copyKeyStatus" class="notice-message" role="status">{{ copyKeyStatus }}</p>
        <div class="import-dialog-actions">
          <button class="refresh-button" type="button" @click="copyCreatedKey">复制密钥</button>
          <button class="primary-button" type="button" @click="closeCreatedKey">我已保存，关闭</button>
        </div>
      </dialog>

      <dialog ref="revokeKeyDialog" class="import-dialog key-dialog" aria-labelledby="revoke-key-title" @cancel.prevent="closeRevokeKeyDialog" @click="($event.target === revokeKeyDialog) && closeRevokeKeyDialog()">
        <div class="import-dialog-heading">
          <div><p class="eyebrow">REVOKE KEY</p><h2 id="revoke-key-title">撤销密钥</h2></div>
          <button class="dialog-close" type="button" aria-label="关闭撤销弹窗" :disabled="revokingKey" @click="closeRevokeKeyDialog">×</button>
        </div>
        <p>确定撤销 <strong>{{ revokeKeyTarget?.name }}</strong>？撤销后使用该密钥的代理请求会立即收到 401，且无法恢复。</p>
        <p v-if="revokeKeyError" class="error-message" role="alert">{{ revokeKeyError }}</p>
        <div class="import-dialog-actions">
          <button class="refresh-button" type="button" :disabled="revokingKey" @click="closeRevokeKeyDialog">取消</button>
          <button class="danger-button" type="button" :disabled="revokingKey" @click="revokeKey">{{ revokingKey ? '正在撤销…' : '确认撤销' }}</button>
        </div>
      </dialog>
    </main>

    <main v-else class="dashboard-main account-main">
      <div class="dashboard-heading">
        <div><h1>账户设置</h1><p class="dashboard-subtitle">当前登录账户：<strong>{{ currentUser }}</strong></p></div>
      </div>
      <p v-if="notice" class="dashboard-message notice-message" role="status">{{ notice }}</p>
      <p v-if="error" class="dashboard-message error-message" role="alert">{{ error }}</p>
      <div class="account-settings-grid">
        <section class="settings-card">
          <div class="settings-card-heading"><span class="settings-icon">◎</span><div><h2>修改管理员用户名</h2></div></div>
          <form class="auth-form" @submit.prevent="changeUsername">
            <label for="new-username">新用户名</label>
            <input id="new-username" v-model.trim="newUsername" name="new-username" type="text" autocomplete="username" minlength="3" maxlength="32" required />
            <label for="rename-password">当前密码</label>
            <input id="rename-password" v-model="renamePassword" name="rename-password" type="password" autocomplete="current-password" maxlength="128" required />
            <button class="primary-button" type="submit" :disabled="busy">{{ busy ? '请稍候…' : '保存用户名' }}</button>
          </form>
        </section>
        <section class="settings-card">
          <div class="settings-card-heading"><span class="settings-icon">⌑</span><div><h2>修改管理员密码</h2><p>更改密码后需要重新登录。</p></div></div>
          <form class="auth-form" @submit.prevent="changePassword">
            <label for="current-password">当前密码</label>
            <input id="current-password" v-model="currentPassword" name="current-password" type="password" autocomplete="current-password" maxlength="128" required />
            <label for="new-password">新密码</label>
            <input id="new-password" v-model="newPassword" name="new-password" type="password" autocomplete="new-password" minlength="8" maxlength="128" required />
            <button class="primary-button" type="submit" :disabled="busy">{{ busy ? '请稍候…' : '更新密码并重新登录' }}</button>
          </form>
        </section>
      </div>
    </main>

    <footer class="page-footer">
      <span>© 2026 Formicary</span>
      <span>{{ currentUser ? 'Chamber 服务控制台' : '管理控制面板' }}</span>
      <button v-if="currentUser" class="logout-link" type="button" :disabled="busy" @click="logout">退出登录</button>
    </footer>
  </div>
</template>
