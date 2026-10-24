# API 密钥

API 密钥用于给 `proxyhttp` 代理服务增加可选的密钥认证。Formicary 负责签发、验证和传递身份；Chamber 自己决定哪些接口要求密钥、哪些接口公开。Chamber 的 `chamber.toml` 不需要增加任何字段。

## 1. 设计边界

- 密钥没有作用域，也没有过期时间。
- 任何有效密钥都可以用于任何 Chamber 的代理请求。
- Formicary 不会在代理层强制某个路径必须带密钥；是否拒绝匿名请求由 Chamber 决定。
- 密钥只适用于 `proxyhttp` 代理请求和 `GET /api/me`。
- Formicary 自己的管理 API（`/api/jobs`、`/api/chambers`、`/api/keys` 等）仍然只接受浏览器登录会话。
- `staticweb` 静态目录不参与密钥认证。

## 2. 创建和管理

在「API 密钥」页面创建：

1. 填写名称。
2. 输入当前管理员密码。
3. 创建后立即复制完整密钥。

完整密钥只显示一次，关闭弹窗后无法再次查看。数据库只保存 SHA-256 哈希。

也可以使用 API：

```http
POST /api/keys
Content-Type: application/json
Cookie: formicary_session=...

{
  "name": "blog-ci",
  "current_password": "当前管理员密码"
}
```

响应：

```json
{
  "key": {
    "id": 1,
    "name": "blog-ci",
    "key_id": "8f3c2a1b4d5e6f708192a3b4",
    "created_at": "2026-10-24T15:30:00+00:00",
    "last_used_at": null,
    "revoked_at": null,
    "status": "active"
  },
  "token": "fmk_8f3c2a1b4d5e6f708192a3b4_...",
  "warning": "这是唯一一次显示完整密钥，请立即保存；该密钥永不过期，只能撤销。"
}
```

列表：

```http
GET /api/keys
Cookie: formicary_session=...
```

撤销：

```http
DELETE /api/keys/{id}
Cookie: formicary_session=...
```

撤销立即生效，使用该密钥的代理请求下一次就会得到 `401`。

## 3. 在请求中携带密钥

```bash
curl -H "Authorization: Bearer $FORMICARY_API_KEY" \
  https://formicary.example/blog/admin/items
```

或：

```bash
curl -H "X-API-Key: $FORMICARY_API_KEY" \
  https://formicary.example/blog/admin/items
```

只有 `Bearer fmk_...` 会被当作 Formicary 密钥；其他 Bearer token 会留给 upstream 或反向代理。`X-API-Key` 始终由 Formicary 消费。

## 4. Chamber 如何使用

### 4.1 通过 Formicary 代理

代理请求带有效密钥时，Formicary 会注入：

```http
X-Formicary-Auth: api_key
X-Formicary-User: admin
X-Formicary-Principal: api-key:8f3c2a1b4d5e6f708192a3b4
X-Formicary-Key-Id: 8f3c2a1b4d5e6f708192a3b4
```

浏览器登录会话会注入：

```http
X-Formicary-Auth: session
X-Formicary-User: admin
X-Formicary-Principal: admin
```

Chamber 可以按接口自行判断：

```js
// 只保护 /admin/*，其他接口保持公开
function requireFormicaryKey(req, res, next) {
  if (req.get('X-Formicary-Auth') !== 'api_key') {
    return res.status(401).json({ error: 'API key required' })
  }
  req.formicary = {
    user: req.get('X-Formicary-User'),
    keyId: req.get('X-Formicary-Key-Id'),
    principal: req.get('X-Formicary-Principal'),
  }
  next()
}

app.use('/admin', requireFormicaryKey)
```

如果需要接受登录会话或密钥中的任意一种：

```js
if (!req.get('X-Formicary-Auth')) {
  return res.status(401).json({ error: 'Authentication required' })
}
```

Chamber 也可以按 `X-Formicary-Key-Id` 维护自己的 key 白名单或角色映射。

### 4.2 不经过 Formicary 代理时主动验证

Chamber 如果直接收到密钥，可以调用：

```http
GET /api/me
Authorization: Bearer fmk_...
```

有效密钥返回：

```json
{
  "authenticated": true,
  "username": "admin",
  "auth": "api_key",
  "key": {
    "id": 1,
    "name": "blog-ci",
    "key_id": "8f3c2a1b4d5e6f708192a3b4"
  }
}
```

无效或已撤销密钥返回 `401`。

## 5. 代理层处理规则

- 带有效密钥：注入上述身份头，剥离密钥后转发。
- 带无效或已撤销密钥：返回 `401`，不转发。
- 没有密钥：保持现有行为，匿名转发；如果带浏览器会话，则注入会话身份。
- 同时带密钥和会话：密钥优先，并剥离 `formicary_session` Cookie。
- 客户端传入的 `X-Formicary-*` 会被剥离，不能伪造身份。
- `Authorization: Bearer fmk_...` 和 `X-API-Key` 不会转发给 Chamber。

## 6. 安全注意事项

- 密钥永不过期，撤销是唯一的关停方式；泄露后应立即撤销。
- 所有密钥权限相同；建议按用途分别签发，便于单独撤销。
- 远程部署必须使用 HTTPS，否则代理请求中的密钥可被嗅探。
- 不要把密钥写入前端代码、仓库、URL 或日志。
- Chamber 的 upstream 必须只允许 Formicary 访问（绑定 `127.0.0.1` 或防火墙），否则客户端可以绕过代理伪造 `X-Formicary-*`。
- Chamber 不应把收到的身份头缓存到超过单次请求的生命周期；撤销后的下一次请求会由 Formicary 拒绝。
- 创建密钥需要当前管理员密码，避免会话被盗后静默签发永久密钥。
