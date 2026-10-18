# Chamber 与 Job 配置规范

本文档描述当前启动加载器使用的配置格式和运行行为。

## 1. 配置目录

通过 `.env` 中的 `FORMICARY_CHAMBERS_DIR` 指定 Chamber 根目录。该变量指向直接包含各 Chamber 子目录的目录；相对路径以进程当前工作目录为基准。未设置时使用 `./chamber`。

```dotenv
FORMICARY_CHAMBERS_DIR=./chamber
```

当前 `make run` 会在启动前导出仓库根目录 `.env` 中的变量。直接运行二进制时，启动器需要先导出这些环境变量；服务读取进程环境。

目录结构：

```text
chamber/
  maintenance/
    chamber.toml
    scripts/
      cleanup.sh
  reporting/
    chamber.toml
```

程序启动时只扫描根目录的直接子目录，不递归扫描。子目录中存在 `chamber.toml` 时，将其作为一个 Chamber 加载；没有该文件的子目录会被忽略。扫描顺序按目录名排序，保证启动行为可预测。

根目录不存在、不可读，或任一 `chamber.toml` 格式或校验失败时，启动应报错退出，避免任务因配置错误而被静默跳过。若暂时不需要任务，创建一个空的 `chamber/` 目录即可。

## 2. `chamber.toml` 格式

每个 Chamber 使用一个 `chamber.toml`。版本 1 的示例：

```toml
[chamber]
name = "maintenance"
schema_version = 1

[[jobs]]
name = "daily-cleanup"
enabled = true
cron = "30 2 * * *"
working_directory = "."
command = "./scripts/cleanup.sh"
timeout = "10m"

[[jobs]]
name = "health-report"
cron = "0 8 * * 1-5"
working_directory = "."
command = "./scripts/health-report.sh"
timeout = "30s"

[staticweb]
directory = "./public"
route = "/assets"
```

规则：

- `[chamber]` 必须存在；`name` 必须非空，并与父目录名一致；`schema_version` 当前必须为 `1`。
- `[[jobs]]` 可以出现多次，也可以不出现。`name` 在同一个 Chamber 内必须唯一且非空。
- 每个 Job 必须提供 `name`、`cron`、`working_directory`、`command` 和 `timeout`。
- `enabled` 可省略，省略时默认为 `true`；设为 `false` 时，Job 会被加载但以暂停状态启动。
- `[staticweb]` 可省略，用于配置一个静态目录及其 URL route；需要多个挂载时使用重复的 `[[staticweb]]`。
- 未识别的字段应视为配置错误，避免拼写错误导致配置被忽略。

## 3. Job 字段

| 字段 | 类型 | 规范 |
| --- | --- | --- |
| `name` | 字符串 | 必填；同一个 Chamber 内唯一；作为任务的人类可读名称。 |
| `enabled` | 布尔值 | 可选；默认 `true`。`false` 表示启动后暂停。 |
| `cron` | 字符串 | 必填；使用标准五段 Unix cron：分钟、小时、月内日期、月份、星期。 |
| `working_directory` | 字符串 | 必填；相对路径以当前 `chamber.toml` 所在目录为基准；也可使用绝对路径。目录必须存在。 |
| `command` | 字符串 | 必填；交给平台 shell 执行的命令文本。 |
| `timeout` | 字符串 | 必填；正整数加单位，单位为 `ms`、`s`、`m` 或 `h`，范围大于 0 且不超过 24 小时。 |

## 4. 静态 Web 字段

`[staticweb]` 或每个 `[[staticweb]]` 项包含以下字段：

| 字段 | 类型 | 规范 |
| --- | --- | --- |
| `directory` | 字符串 | 必填；静态文件目录。相对路径以当前 `chamber.toml` 所在目录为基准；也可使用绝对路径。目录必须存在。 |
| `route` | 字符串 | 必填；URL 挂载前缀，例如 `/assets`。可省略开头和结尾的 `/`。跨所有 Chamber 的规范化 route 必须唯一；`/api` 和 `/ui` 前缀保留给应用。 |

启动时，Chamber 加载器解析并校验目录和 route，然后将其注册到应用的静态目录挂载表。访问 `/assets/logo.svg` 会读取 `directory/logo.svg`；访问挂载前缀或目录路径时会尝试返回该目录下的 `index.html`。只响应 `GET` 和 `HEAD`。重叠 route 按最长前缀优先匹配。目录越界路径及指向挂载目录外的符号链接不会被提供。

挂载 route 在所有已加载的 Chamber 之间必须唯一。运行期间可通过共享的 `App` 句柄动态增加或注销挂载；这类运行时改动不会写回 `chamber.toml`。修改 TOML 配置仍需重启服务。

## 5. Cron 时间

规范只接受五段 Unix cron 表达式。调度使用服务器本地时区；版本 1 不支持单个 Job 指定时区。表达式在启动时校验，非法表达式会导致启动失败。

## 6. 工作目录与命令

相对 `working_directory` 从 Chamber 目录解析。例如 `"."` 表示包含 `chamber.toml` 的目录。`command` 由 `ShellRunner` 作为 shell 文本执行，因此可以使用管道、重定向和 shell 语法；命令配置必须来自可信任的本地文件，不应由不可信用户输入生成。

`timeout` 是单次命令运行的墙钟上限。到达上限后，runner 会终止命令及其子进程，并记录超时。标准输出和标准错误会写入服务日志；非零退出码会记录，但不会自动暂停 Job。

## 7. 启动与运行行为

- 配置只在程序启动时加载。修改 TOML 后需要重启程序；版本 1 不定义热重载。
- `JobService` 在进程内是全局单例；已加载的 Jobs 由 Chamber 持有共享引用。
- 每个 Job 同一时间最多运行一个命令。上一次执行尚未结束时，不会并发启动同一 Job 的下一次执行；错过的触发时间不会补跑。
- 暂停/启动状态是运行时状态，不回写 TOML。重启后状态重新由 `enabled` 决定。
- TOML 里的 `enabled = false` 会使 Job 保留在配置和列表中，但不执行定时命令。

## 8. 校验失败

以下情况应报告包含 Chamber 路径及字段位置的错误，并终止启动：无法读取 TOML、版本不支持、Chamber 名称与目录名不一致、Job 名称重复或为空、字段缺失或类型错误、cron 无效、超时格式无效、工作目录不存在、静态目录不存在或不是目录、静态 route 无效或重复，或出现未知字段。
