# 从 GitHub 导入 Chamber

在「Chamber 管理」页面点击「从 GitHub 导入」按钮，在弹窗中填写：

- GitHub 仓库：公开或私有仓库的 `owner/repo` 或 `https://github.com/owner/repo` 地址。导入私有仓库前，点击「登录 GitHub」，按提示在 GitHub 输入授权码。授权后可查询分支、导入或升级。
- 目标分支：填写仓库地址后自动获取分支列表并选中默认分支，可在下拉框切换；获取失败可点击「重新获取」。分支加载完成后才能提交导入。
- 下载深度：默认 `1`，仅下载最近一次提交；可填写其他正整数，留空下载所选分支的完整历史。

## 配置私有仓库访问

1. 在 GitHub 注册一个 **GitHub App**，启用 **Device Flow**，将仓库的 **Contents** 权限设为 **Read-only**。无需配置回调地址或客户端密钥。 [GitHub App 注册文档](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app)
2. 将这个 App 安装到私有仓库所属的个人账号或组织，选择要导入的仓库。登录 GitHub 的账号也需要拥有这些仓库的读取权限。 [GitHub App 用户授权说明](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-with-a-github-app-on-behalf-of-a-user)
3. 在 Formicary 的服务器环境中设置 `FORMICARY_GITHUB_CLIENT_ID` 为 App 设置页上的 **Client ID**，然后重启 Formicary。

公开仓库可直接导入。GitHub 授权绑定当前 Formicary 登录会话，访问令牌保存在服务端内存。可在导入或升级界面断开授权，令牌过期或服务重启后需重新授权。

## 仓库要求和接口

仓库根目录必须包含有效的 `chamber.toml`。系统自动下载到配置的 Chamber 根目录下，以 `[chamber].name` 创建子目录，无需手动填写目录；默认路径为 `./chamber/<名称>`，可通过 `FORMICARY_CHAMBERS_DIR` 配置根目录。同名 Chamber 或目录已存在时会拒绝导入，保留已有内容。导入成功后，新 Chamber 立即出现在列表中，配置的 Job、静态目录和 HTTP 代理会加载，启用的任务和代理脚本会运行。配置引用的目录必须已存在于仓库中。

服务器需要安装 Git 并能够访问 GitHub。下载超过五分钟会终止；导入失败会清理新建目录。下载保留 `.git`。

API：登录后向 `POST /api/chambers/import` 提交 JSON：

```json
{"repository":"owner/repo","branch":"main","depth":1}
```

`depth: null` 表示完整历史。成功返回 `201` 和新 Chamber 的 `name`；无效输入或导入失败返回错误信息。

提交后弹窗显示不定进度条和当前处理状态。导入期间不可重复提交或关闭弹窗，成功后点击「完成」，失败后可修改参数重试。进度条不显示估算的下载百分比。

分支查询接口：登录后向 `POST /api/chambers/github/branches` 提交 `{"repository":"owner/repo"}`，返回 `branches` 和 `default_branch`。查询通过 Git 读取远端引用，不下载仓库内容，超时为 30 秒。

GitHub 授权接口为 `/api/chambers/github/account`，需要 Formicary 登录会话。使用 `GET` 查询状态，`POST` 发起授权，`PATCH` 根据返回的 `retry_after` 秒数轮询，`DELETE` 断开授权。分支查询、导入和升级通过同一 Formicary 会话读取授权。
