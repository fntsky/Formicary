<script setup>
const props = defineProps({ authorization: { type: Object, required: true }, disabled: Boolean })
const { account, busy, error, update } = props.authorization
</script>

<template>
  <div class="github-account">
    <div class="github-account-row">
      <span v-if="account?.status === 'connected'" role="status">已连接 GitHub <strong>@{{ account.login }}</strong></span>
      <span v-else>私有仓库需要 GitHub 授权</span>
      <button v-if="account?.status === 'connected'" class="inline-link" type="button" :disabled="disabled || busy" @click="update('DELETE')">断开授权</button>
      <button v-else-if="account?.status === 'pending'" class="inline-link" type="button" :disabled="disabled || busy" @click="update('DELETE')">取消授权</button>
      <button v-else-if="account?.status === 'disconnected'" class="refresh-button" type="button" :disabled="disabled || busy" @click="update('POST')">{{ busy ? '正在获取授权码…' : '登录 GitHub' }}</button>
      <button v-else-if="!account && error" class="inline-link" type="button" :disabled="disabled || busy" @click="update('GET')">重试</button>
    </div>
    <p v-if="account?.status === 'unconfigured'">管理员需在服务器设置 <code>FORMICARY_GITHUB_CLIENT_ID</code>。</p>
    <template v-if="account?.status === 'pending'">
      <p>请在 GitHub 输入授权码 <strong class="github-device-code">{{ account.user_code }}</strong></p>
      <p><a class="inline-link" :href="account.verification_url" target="_blank" rel="noopener noreferrer">打开 GitHub 授权页面 ↗</a></p>
      <p role="status">等待 GitHub 确认授权…</p>
    </template>
    <p v-if="error" class="error-message" role="alert">{{ error }}</p>
  </div>
</template>
