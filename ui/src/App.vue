<script setup>
import { onMounted, ref } from 'vue'

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

async function readResponse(response) {
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(data.error || '请求失败，请稍后重试。')
  }
  return data
}

async function postJson(endpoint, payload) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify(payload),
  })
  return readResponse(response)
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
    const response = await fetch('/api/logout', {
      method: 'POST',
      credentials: 'same-origin',
    })
    await readResponse(response)
    currentUser.value = ''
    password.value = ''
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

onMounted(async () => {
  try {
    const response = await fetch('/api/me', { credentials: 'same-origin' })
    if (response.ok) {
      const data = await response.json()
      currentUser.value = data.username
      username.value = data.username
      newUsername.value = data.username
    }
  } catch {
    error.value = '无法连接到管理员服务。'
  }
})
</script>

<template>
  <div class="page-shell">
    <header class="topbar">
      <a class="brand" href="/ui/" aria-label="Formicary 控制面板">
        <span class="brand-mark" aria-hidden="true">F</span>
        <span>Formicary</span>
      </a>
      <span class="topbar-note">管理员控制面板</span>
    </header>

    <main class="main-content">
      <section class="welcome-panel" aria-label="控制面板介绍">
        <div class="welcome-kicker"><span></span> FORMICARY CONTROL PANEL</div>
        <h1>管理你的<br />Formicary 服务。</h1>
        <p>登录后修改管理员账户信息。</p>
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

      <section class="auth-card" :class="{ 'dashboard-card': currentUser }" aria-labelledby="form-title">
        <template v-if="currentUser">
          <p class="eyebrow">ADMIN ACCOUNT</p>
          <h2 id="form-title">管理员设置</h2>
          <p class="card-description">当前登录账户：<strong>{{ currentUser }}</strong></p>

          <p v-if="notice" class="notice-message" role="status">{{ notice }}</p>
          <p v-if="error" class="error-message" role="alert">{{ error }}</p>

          <section class="settings-section" aria-labelledby="username-title">
            <h3 id="username-title">修改管理员用户名</h3>
            <form class="auth-form" @submit.prevent="changeUsername">
              <label for="new-username">新用户名</label>
              <input
                id="new-username"
                v-model.trim="newUsername"
                name="new-username"
                type="text"
                autocomplete="username"
                minlength="3"
                maxlength="32"
                required
              />

              <label for="rename-password">当前密码</label>
              <input
                id="rename-password"
                v-model="renamePassword"
                name="rename-password"
                type="password"
                autocomplete="current-password"
                maxlength="128"
                required
              />

              <button class="primary-button" type="submit" :disabled="busy">
                {{ busy ? '请稍候…' : '保存用户名' }}
              </button>
            </form>
          </section>

          <section class="settings-section" aria-labelledby="password-title">
            <h3 id="password-title">修改管理员密码</h3>
            <form class="auth-form" @submit.prevent="changePassword">
              <label for="current-password">当前密码</label>
              <input
                id="current-password"
                v-model="currentPassword"
                name="current-password"
                type="password"
                autocomplete="current-password"
                maxlength="128"
                required
              />

              <label for="new-password">新密码</label>
              <input
                id="new-password"
                v-model="newPassword"
                name="new-password"
                type="password"
                autocomplete="new-password"
                minlength="8"
                maxlength="128"
                required
              />

              <button class="primary-button" type="submit" :disabled="busy">
                {{ busy ? '请稍候…' : '更新密码并重新登录' }}
              </button>
            </form>
          </section>

          <button class="secondary-button" type="button" :disabled="busy" @click="logout">
            退出登录
          </button>
        </template>

        <template v-else>
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
        </template>

        <div class="card-footer"><span class="lock-icon" aria-hidden="true">⌑</span> 管理员账户安全设置</div>
      </section>
    </main>

    <footer class="page-footer">
      <span>© 2026 Formicary</span>
      <span>管理控制面板</span>
    </footer>
  </div>
</template>
