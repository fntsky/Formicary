<script setup>
import { onMounted, ref } from 'vue'

const mode = ref('login')
const username = ref('')
const password = ref('')
const currentUser = ref('')
const error = ref('')
const busy = ref(false)

const isRegistering = () => mode.value === 'register'

function switchMode(nextMode) {
  mode.value = nextMode
  error.value = ''
  password.value = ''
}

async function readResponse(response) {
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(data.error || '请求失败，请稍后重试。')
  }
  return data
}

async function submit() {
  error.value = ''
  busy.value = true
  try {
    const endpoint = isRegistering() ? '/api/register' : '/api/login'
    const response = await fetch(endpoint, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'same-origin',
      body: JSON.stringify({ username: username.value, password: password.value }),
    })
    const data = await readResponse(response)
    currentUser.value = data.username
    password.value = ''
  } catch (requestError) {
    error.value = requestError.message
  } finally {
    busy.value = false
  }
}

async function logout() {
  busy.value = true
  try {
    const response = await fetch('/api/logout', { method: 'POST', credentials: 'same-origin' })
    await readResponse(response)
    currentUser.value = ''
    switchMode('login')
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
    }
  } catch {
    error.value = '无法连接到登录服务。'
  }
})
</script>

<template>
  <div class="page-shell">
    <header class="topbar">
      <a class="brand" href="/ui/" aria-label="Formicary 首页">
        <span class="brand-mark" aria-hidden="true">F</span>
        <span>Formicary</span>
      </a>
      <span class="topbar-note">安全访问你的工作空间</span>
    </header>

    <main class="main-content">
      <section class="welcome-panel" aria-label="产品介绍">
        <div class="welcome-kicker"><span></span> WORKSPACE ACCESS</div>
        <h1>让每一次协作<br />都有清晰的起点。</h1>
        <p>登录后继续访问你的 Formicary 工作空间。</p>
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
        <template v-if="currentUser">
          <div class="success-icon" aria-hidden="true">✓</div>
          <p class="eyebrow">SIGNED IN</p>
          <h2 id="form-title">欢迎回来</h2>
          <p class="card-description">当前登录账户：<strong>{{ currentUser }}</strong></p>
          <button class="primary-button" type="button" :disabled="busy" @click="logout">
            {{ busy ? '正在退出…' : '退出登录' }}
          </button>
        </template>

        <template v-else>
          <p class="eyebrow">{{ isRegistering() ? 'CREATE ACCOUNT' : 'ACCOUNT ACCESS' }}</p>
          <h2 id="form-title">{{ isRegistering() ? '创建账户' : '登录账户' }}</h2>
          <p class="card-description">
            {{ isRegistering() ? '注册一个账户，开始使用 Formicary。' : '输入你的账户信息以继续。' }}
          </p>

          <form class="auth-form" @submit.prevent="submit">
            <label for="username">用户名</label>
            <input
              id="username"
              v-model.trim="username"
              name="username"
              type="text"
              autocomplete="username"
              placeholder="例如：alex"
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
              :autocomplete="isRegistering() ? 'new-password' : 'current-password'"
              placeholder="至少 8 个字符"
              :minlength="isRegistering() ? 8 : 1"
              maxlength="128"
              required
            />

            <p v-if="error" class="error-message" role="alert">{{ error }}</p>

            <button class="primary-button" type="submit" :disabled="busy">
              {{ busy ? '请稍候…' : isRegistering() ? '创建账户' : '登录' }}
              <span aria-hidden="true">→</span>
            </button>
          </form>

          <p class="switch-prompt">
            {{ isRegistering() ? '已经有账户？' : '还没有账户？' }}
            <button
              class="text-button"
              type="button"
              @click="switchMode(isRegistering() ? 'login' : 'register')"
            >
              {{ isRegistering() ? '返回登录' : '创建账户' }}
            </button>
          </p>
        </template>

        <div class="card-footer"><span class="lock-icon" aria-hidden="true">⌑</span> 账户信息安全存储</div>
      </section>
    </main>

    <footer class="page-footer">
      <span>© 2026 Formicary</span>
      <span>简单 · 清晰 · 专注</span>
    </footer>
  </div>
</template>
