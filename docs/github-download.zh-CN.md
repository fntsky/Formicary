# 从 GitHub 导入 Chamber

在「Chamber 管理」页面点击「从 GitHub 导入」按钮，在弹窗中填写：

- GitHub 仓库：公开仓库的 `owner/repo` 或 `https://github.com/owner/repo` 地址。
- 目标分支：填写仓库地址后自动获取分支列表并选中默认分支，可在下拉框切换；获取失败可点击「重新获取」。分支加载完成后才能提交导入。
- 下载深度：默认 `1`，仅下载最近一次提交；可填写其他正整数，留空下载所选分支的完整历史。

仓库根目录必须包含有效的 `chamber.toml`。系统自动下载到配置的 Chamber 根目录下，以 `[chamber].name` 创建子目录，无需手动填写目录；默认路径为 `./chamber/<名称>`，可通过 `FORMICARY_CHAMBERS_DIR` 配置根目录。同名 Chamber 或目录已存在时会拒绝导入，保留已有内容。导入成功后，新 Chamber 立即出现在列表中，配置的 Job、静态目录和 HTTP 代理会加载，启用的任务和代理脚本会运行。配置引用的目录必须已存在于仓库中。

服务器需要安装 Git 并能够访问 GitHub。下载超过五分钟会终止；导入失败会清理新建目录。下载保留 `.git`。

API：登录后向 `POST /api/chambers/import` 提交 JSON：

```json
{"repository":"owner/repo","branch":"main","depth":1}
```

`depth: null` 表示完整历史。成功返回 `201` 和新 Chamber 的 `name`；无效输入或导入失败返回错误信息。

提交后弹窗显示不定进度条和当前处理状态。导入期间不可重复提交或关闭弹窗，成功后点击「完成」，失败后可修改参数重试。进度条不显示估算的下载百分比。

分支查询接口：登录后向 `POST /api/chambers/github/branches` 提交 `{"repository":"owner/repo"}`，返回 `branches` 和 `default_branch`。查询通过 Git 读取远端引用，不下载仓库内容，超时为 30 秒。
