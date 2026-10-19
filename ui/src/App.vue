<script setup>
import { computed, onMounted, ref } from 'vue'

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
const chamberTab = ref('overview')
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
          <button :class="{ selected: activePage === 'chambers' }" @click="activePage = 'chambers'">服务管理</button>
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
          <p class="eyebrow">SERVICE OVERVIEW</p>
          <h1>Chamber 管理</h1>
          <p class="dashboard-subtitle">查看已加载的 Chamber、定时任务和服务挂载。</p>
        </div>
        <button class="refresh-button" type="button" :disabled="refreshing" @click="refreshChambers">
          <span aria-hidden="true">↻</span>{{ refreshing ? '正在刷新' : '刷新数据' }}
        </button>
      </div>

      <p v-if="chamberError" class="dashboard-message error-message" role="alert">{{ chamberError }}</p>
      <p v-if="chamberNotice" class="dashboard-message notice-message" role="status">{{ chamberNotice }}</p>

      <section class="metric-grid" aria-label="服务统计">
        <article class="metric-card">
          <span class="metric-icon green-icon" aria-hidden="true">◈</span>
          <div><p>已加载 Chamber</p><strong>{{ chambers.length }}</strong></div>
        </article>
        <article class="metric-card">
          <span class="metric-icon blue-icon" aria-hidden="true">◷</span>
          <div><p>定时任务</p><strong>{{ totalJobs }}</strong><small>{{ enabledJobs }} 个已启用</small></div>
        </article>
        <article class="metric-card">
          <span class="metric-icon amber-icon" aria-hidden="true">⌘</span>
          <div><p>服务挂载</p><strong>{{ totalMounts }}</strong><small>静态目录和 HTTP 代理</small></div>
        </article>
      </section>

      <section class="chamber-layout">
        <aside class="chamber-list-panel" aria-label="Chamber 列表">
          <div class="panel-heading">
            <div><h2>Chambers</h2><p>配置目录中的服务模块</p></div>
            <span class="count-pill">{{ chambers.length }}</span>
          </div>

          <div v-if="loadingChambers && !chambers.length" class="panel-empty">正在读取 Chamber…</div>
          <div v-else-if="!chambers.length" class="panel-empty">
            <span class="empty-mark" aria-hidden="true">◈</span>
            <strong>还没有 Chamber</strong>
            <span>在配置目录中添加 Chamber 后会显示在这里。</span>
          </div>
          <div v-else class="chamber-list">
            <button
              v-for="item in chambers"
              :key="item.name"
              class="chamber-list-item"
              :class="{ active: selectedChamber === item.name }"
              type="button"
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

          <div class="list-panel-footer"><span class="live-dot"></span> 配置已加载</div>
        </aside>

        <section class="chamber-detail-panel" aria-live="polite">
          <div v-if="loadingChamber" class="detail-empty">正在读取 Chamber 详情…</div>
          <div v-else-if="!chamberDetails" class="detail-empty">
            <span class="empty-mark" aria-hidden="true">◈</span>
            <strong>{{ chambers.length ? '选择一个 Chamber' : '等待 Chamber 配置' }}</strong>
            <span>{{ chambers.length ? '从左侧列表选择服务以查看详情。' : '检测到 Chamber 后，详情和任务会显示在这里。' }}</span>
          </div>
          <template v-else>
            <div class="detail-heading">
              <div class="detail-title-group">
                <span class="detail-avatar" aria-hidden="true">{{ chamberDetails.name.slice(0, 1).toUpperCase() }}</span>
                <div>
                  <div class="detail-name-line"><h2>{{ chamberDetails.name }}</h2><span class="loaded-badge"><i></i>已加载</span></div>
                  <p class="path-label"><span aria-hidden="true">⌂</span>{{ chamberDetails.directory }}</p>
                </div>
              </div>
              <span class="config-label">SCHEMA V1</span>
            </div>

            <nav class="detail-tabs" aria-label="Chamber 详情">
              <button :class="{ selected: chamberTab === 'overview' }" @click="chamberTab = 'overview'">概览</button>
              <button :class="{ selected: chamberTab === 'jobs' }" @click="chamberTab = 'jobs'">定时任务 <span>{{ chamberDetails.job_count }}</span></button>
              <button :class="{ selected: chamberTab === 'mounts' }" @click="chamberTab = 'mounts'">服务挂载 <span>{{ chamberDetails.static_web.length + chamberDetails.proxy_http.length }}</span></button>
            </nav>

            <div v-if="chamberTab === 'overview'" class="detail-body">
              <div class="overview-stats">
                <article><span class="mini-stat-icon">◷</span><div><span>定时任务</span><strong>{{ chamberDetails.job_count }}</strong></div></article>
                <article><span class="mini-stat-icon">✓</span><div><span>已启用</span><strong>{{ chamberDetails.enabled_job_count }}</strong></div></article>
                <article><span class="mini-stat-icon">⌘</span><div><span>静态挂载</span><strong>{{ chamberDetails.static_web.length }}</strong></div></article>
                <article><span class="mini-stat-icon">⇄</span><div><span>HTTP 代理</span><strong>{{ chamberDetails.proxy_http.length }}</strong></div></article>
              </div>

              <div class="overview-grid">
                <section class="content-card">
                  <div class="content-card-heading"><div><h3>任务预览</h3><p>此 Chamber 中配置的定时任务</p></div><button class="inline-link" @click="chamberTab = 'jobs'">查看全部 <span>→</span></button></div>
                  <div v-if="!chamberDetails.jobs.length" class="small-empty">暂未配置定时任务。</div>
                  <div v-else class="compact-job-list">
                    <div v-for="job in chamberDetails.jobs.slice(0, 4)" :key="job.id" class="compact-job-row">
                      <span class="job-state-dot" :class="{ paused: !job.enabled }"></span>
                      <div><strong>{{ job.name }}</strong><span>{{ job.cron }}</span></div>
                      <span class="state-text" :class="{ paused: !job.enabled }">{{ job.enabled ? '已启用' : '已暂停' }}</span>
                    </div>
                  </div>
                </section>

                <section class="content-card">
                  <div class="content-card-heading"><div><h3>服务挂载</h3><p>HTTP 路由和本地目录</p></div><button class="inline-link" @click="chamberTab = 'mounts'">查看全部 <span>→</span></button></div>
                  <div v-if="!chamberDetails.static_web.length && !chamberDetails.proxy_http.length" class="small-empty">暂未配置服务挂载。</div>
                  <div v-else class="compact-mount-list">
                    <div v-for="mount in chamberDetails.static_web.slice(0, 2)" :key="`static-${mount.route}`" class="compact-mount-row">
                      <span class="mount-type-icon static-mount-icon">▤</span><div><strong>{{ mount.route }}</strong><span>静态目录</span></div><span class="mount-arrow">↗</span>
                    </div>
                    <div v-for="mount in chamberDetails.proxy_http.slice(0, 2)" :key="`proxy-${mount.route}`" class="compact-mount-row">
                      <span class="mount-type-icon proxy-mount-icon">⇄</span><div><strong>{{ mount.route }}</strong><span>HTTP 代理</span></div><span class="mount-arrow">↗</span>
                    </div>
                  </div>
                </section>
              </div>
            </div>

            <div v-else-if="chamberTab === 'jobs'" class="detail-body">
              <div class="tab-content-heading"><div><h3>定时任务</h3><p>查看任务配置，并即时暂停或恢复调度。</p></div><span class="count-pill">{{ chamberDetails.jobs.length }} 项</span></div>
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
              <div class="tab-content-heading"><div><h3>服务挂载</h3><p>Chamber 加载的静态目录和 HTTP 代理路由。</p></div><span class="count-pill">{{ chamberDetails.static_web.length + chamberDetails.proxy_http.length }} 项</span></div>
              <div class="mount-groups">
                <section class="mount-group">
                  <div class="mount-group-heading"><span class="mount-type-icon static-mount-icon">▤</span><div><h4>静态目录</h4><p>直接从本地目录提供文件</p></div><span class="count-pill">{{ chamberDetails.static_web.length }}</span></div>
                  <div v-if="!chamberDetails.static_web.length" class="small-empty">没有静态目录挂载。</div>
                  <div v-for="mount in chamberDetails.static_web" :key="mount.route" class="mount-card">
                    <div class="mount-route-line"><span class="route-method get-method">GET</span><strong>{{ mount.route }}</strong></div>
                    <p><span>目录</span><code>{{ mount.directory }}</code></p>
                  </div>
                </section>
                <section class="mount-group">
                  <div class="mount-group-heading"><span class="mount-type-icon proxy-mount-icon">⇄</span><div><h4>HTTP 代理</h4><p>转发到本地服务</p></div><span class="count-pill">{{ chamberDetails.proxy_http.length }}</span></div>
                  <div v-if="!chamberDetails.proxy_http.length" class="small-empty">没有 HTTP 代理挂载。</div>
                  <div v-for="mount in chamberDetails.proxy_http" :key="mount.route" class="mount-card">
                    <div class="mount-route-line"><span class="route-method proxy-method">PROXY</span><strong>{{ mount.route }}</strong></div>
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

    <main v-else class="dashboard-main account-main">
      <div class="dashboard-heading">
        <div><p class="eyebrow">ADMIN PROFILE</p><h1>账户设置</h1><p class="dashboard-subtitle">当前登录账户：<strong>{{ currentUser }}</strong></p></div>
      </div>
      <p v-if="notice" class="dashboard-message notice-message" role="status">{{ notice }}</p>
      <p v-if="error" class="dashboard-message error-message" role="alert">{{ error }}</p>
      <div class="account-settings-grid">
        <section class="settings-card">
          <div class="settings-card-heading"><span class="settings-icon">◎</span><div><h2>修改管理员用户名</h2><p>更新用于登录控制面板的账户名称。</p></div></div>
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
