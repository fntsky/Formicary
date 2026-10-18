# 静态目录挂载

默认的 `app::run()` 使用共享的 `App::global()` 实例。程序内的其他模块可以通过它在服务器运行期间动态注册和注销静态目录：

```rust
let app = app::App::global();

// 运行期间新增挂载
app.register_static_directory("./public", "/assets")?;

// 运行期间移除挂载；返回 false 表示该前缀未注册
let removed = app.unregister_static_directory("/assets")?;
```

注册后，`./public/logo.svg` 可通过 `/assets/logo.svg` 访问。变更对后续请求生效；注销时已经开始处理的请求可以完成。请求挂载前缀本身或目录路径时，服务会尝试返回该目录下的 `index.html`。只响应 `GET` 和 `HEAD` 请求；其他方法仍由应用返回 `405 Method Not Allowed`。

规则：

- 相对目录路径以进程当前工作目录为基准。目录必须在注册时存在且为目录。
- 前缀可以写作 `assets` 或 `/assets`；服务会规范化为 `/assets`。前缀可包含多个路径段。
- `/api` 和 `/ui` 是保留前缀；同一个前缀不能重复注册。重叠前缀优先匹配更长的前缀。
- `..`、`.` 和反斜杠路径段会被拒绝。服务会解析文件的规范路径，并拒绝访问挂载目录之外的符号链接目标。
- 根据文件扩展名设置常见的 `Content-Type`；无法识别的类型使用 `application/octet-stream`。
- 目录或前缀无效时，`register_static_directory` 返回 `io::Error`；`unregister_static_directory` 返回是否成功移除。`App::global()` 返回默认服务器使用的共享实例，任意模块都可以通过该实例在运行期间修改挂载。
