# 启动与装配

本模块维护宿主薄启动入口，装配框架和业务服务。Harness 提供方统一通过 `remote-node` 与独立节点接入；宿主不编译或启动供应商私有进程。装配范围还包含可信配置声明的宿主侧插件与其他外部节点绑定，但不提供节点发行构建。

当前已有模拟启动、[Harness 入口](harness_cli.rs)、[审批修复入口](repair_cli.rs)、可选 HTTP 服务和 Tauri Desktop 薄启动。`demo`/`inspect` 保持封闭模拟；`harness` 调用文本与项目服务；`repair` 装配执行/审批实例、持久授权和 Windows 白名单文本动作。Desktop 与 Web 复用 [interfaces](../interfaces/README.md) 中同一组静态资产，并通过同一 HTTP API 连接业务服务。目标专属外部动作、自动节点重连、安装市场与完整自愈仍未实现。

## 共享宿主装配

[host.rs](host.rs) 提供 `HostConfig` 与 `HostRuntime`，供 Harness CLI、修复 CLI 和 HTTP 控制台复用。`configuration.rs` 集中加载活动 Harness、修复、扩展与受保护路径配置；HTTP 不调用 CLI 私有实现。CLI 与 HTTP 各自保留参数、传输认证、展示和业务回执责任。

可选 `MonitoringHostConfig` 把监控引擎与持久故障服务加入共同生命周期，依赖已发布的扩展注册表。监控服务在恢复检查点后发布，关闭先取消轮询并排空，再关闭提供方；具体配置见[监控说明](../../docs/monitoring.md)。

运行时提供方由框架实例发布 `extensions.registry@1[host]` 和 `harness.registry@1[host]`。Harness 模块声明对扩展注册表的依赖；初始化和关闭顺序由同一 `framework::Runtime` 校验并驱动。只有实例完成初始化后才发布服务。`harness list` 复用相同工厂构造逻辑检查装配条件，其暂存检查对象不接受业务调用。

`HostRuntime::shutdown` 同步禁止新派发并请求取消，等待在途监督任务后撤销框架服务。30 秒排空未完成时返回错误并保留运行时，后续仍可继续关闭；超时不代表旧调用已经停止。注册表关闭不替代审批结果持久化或远端副作用核对。

共享层覆盖提供方注册、监控与故障服务、类型化服务发布和生命周期；模拟任务、审批会话与 HTTP 传输回执仍由所属模块维护，尚无全局正式任务调度或任意在线重装配。实现与验收范围见[实现状态](../../docs/implementation-status.md)。

## UI 前端启动

Web 构建目标生成静态目录，可由部署方托管，也可由同包 `server,web-ui` 的 `serve` 入口提供页面和 API。Desktop 目标由 `recuvora-desktop` 创建 Tauri 窗口并加载 `src/interfaces/ui/public/`；壳层不注册业务 command，不持有任务、授权或 Harness 状态。

项目仍是一个 Cargo 包。默认 `cli` feature 保留 `recuvora` CLI；`desktop-ui` 启用 Tauri、隐含 `web-ui` 并开放 Desktop 目标。使用 [build-ui.ps1](../../scripts/build-ui.ps1) 的 `Web`、`Desktop` 或 `All` 目标构建，所有输出必须写到源码目录外。两个载体展示同一页面与功能；页面默认读取可信 API，未连接时显示不可用，显式演示模式始终禁用提交。

## 审批修复入口

`recuvora repair --help` 列出 `run`、`inspect`、`approve`、`deny`、`revoke`、`apply`、`reconcile`。配置示例见 [repair profile](../../profiles/repair.local.example.json)，完整用法、退出码、范围与恢复见[审批说明](../../docs/approval.md)。语法解析在 interfaces，流程和授权在 recovery，文件动作在 actions。

活动配置由本机操作员提供明确审批委托，并保存在模型写入范围外。执行与审批均采用 Hidden 会话；审批独立无工具，使用专用容量。人工批准只记录决定，`apply` 重新检查后执行一次，`reconcile` 只核验内容。进程持有状态写锁，Ctrl-C 后等待在途回调收尾；本机 CLI 依赖操作系统用户身份，HTTP API 使用独立配置的单操作员令牌与权限，尚无多用户账户系统。

修复配置引用的扩展清单也必须位于源码和修复目标范围外。派发前保护本地扩展可执行文件、安装目录、进程工作目录及显式本地配置参数，拒绝与修复目标交叠；SSH 远端命令参数保持远端语义，由节点保护自身安装和配置。

## 职责与边界

- 读取选定 profile 和本地管理配置，解析宿主组合、服务提供方、插件实例、外部节点绑定与依赖作用域。
- 校验版本、必需依赖、提供者冲突、运行边界和实际可用能力。
- 按依赖顺序初始化服务，完成持久恢复与授权检查后开放执行派发。
- 受控关闭和重启时先停止新工作，再排空在途操作并释放资源。

[组合模板](../../profiles/README.md)描述宿主装配；[框架](../framework/README.md)实现服务与生命周期；[自愈业务](../recovery/README.md)提供任务和默认流程；[节点管理](../nodes/README.md)负责连接与能力绑定。节点是独立开发、构建和发布的外部产品，其依赖安装、提供方进程与本机监督由节点项目负责。即使同机部署，节点也不变成宿主内置启动角色。

配置只能选择已编入宿主的内置实现或经管理流程允许接入的外部扩展。安装、启用、能力权限与任务授权分别检查；组合选择不产生授权。第三方宿主侧插件可独立增加受限只读业务契约与消费逻辑，默认进程外；宿主负责自己启动的插件进程，外部节点负责自身进程。断线不能证明远端已回收。

缺失授权、持久服务、必需插件或兼容契约时，相关能力保持关闭。启动成功也不证明目标健康或桌面权限可用。运行时修复无权改变 Recuvora 的启用配置和运行组合。

## Harness 使用入口

`harness` 命令只使用宿主内置的 `remote-node` 代理，[interfaces](../interfaces/README.md)负责参数校验、服务调用和 JSON 展示。独立文本 CLI 不注册宿主工具，也不使用模拟引擎或模拟授权。

### 1. 加载配置并查看实例

把[外部 Harness 示例](../../profiles/harnesses.remote.example.json)和[扩展连接示例](../../profiles/extensions.example.json)复制到源码目录外，按已安装节点调整路径、参数和工作目录，并设置活动配置路径：

```powershell
cargo run --locked -- harness --help
cargo run --locked -- harness list --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG
```

配置中的 Harness 地址、节点 ID 和工作区 ID 必须一致。程序不会安装节点、修改全局 PATH 或写回活动配置。`list` 校验结构、扩展握手和本地装配条件，输出默认项、实例和工作区，以及 `configured`、`disabled` 或 `unavailable`；它不验证节点内的提供方登录或真实模型调用。装配失败时仍输出完整列表与原因，并返回退出码 1。

Windows gnullvm 调试程序需要匹配工具链的运行库。构建目录或启动进程 PATH 可提供所需 DLL；源码目录不得保存运行库或构建产物。

### 2. 选择工作区、项目并调用

先选择 Harness 和节点工作区，再通过同一实例探测项目。省略 `--harness` 时只使用配置声明的默认项。

```powershell
cargo run --locked -- harness projects --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --harness harness-external --workspace work
cargo run --locked -- harness run --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --harness harness-external --workspace work --visibility client --project $env:RECUVORA_NATIVE_PROJECT_ID --prompt "仅根据本提示回复：连接成功"
cargo run --locked -- harness run --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --workspace work --visibility hidden --no-project --prompt-file $env:RECUVORA_PROMPT_FILE
```

`RECUVORA_NATIVE_PROJECT_ID` 使用上一步由同一实例返回的 ID。指定 `--project` 时会再次探测并核验，失败不会自动改成无项目调用。`--no-project` 明确不请求原生项目，但不保证独立客户端不会按自身规则归组。必须显式选择 `--visibility client|hidden` 和 `--project ID|--no-project`；`hidden` 不能组合原生项目。

新建项目是独立命令，不随文本调用自动执行：

```powershell
cargo run --locked -- harness create-project --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --harness harness-external --workspace work --name "目标项目" --idempotency-key $env:RECUVORA_PROJECT_REQUEST_KEY
```

稳定请求键只用于关联本次创建。收到未知结果后先核验提供方状态，不能假定复用请求键就保证外部协议去重。文本输入在 `--prompt` 和 `--prompt-file` 中二选一；文件必须是非空 UTF-8，最大 64 KiB；可选 `--model`。调用前由用户决定哪些内容可以交给该提供方。

### 3. 结果、超时与取消

成功结果以单行 JSON 写入 stdout，包含实例、会话/线程 ID、节点工作区、可见性、原生项目 ID、客户端归组状态和最终文本。所有外部字符串经过 JSON 转义，不把模型输出解释为终端命令。客户端归组可能保持 `unverified`。

操作错误以 JSON 写入 stderr，含稳定分类、可读原因和 `auto_retry: false`。参数错误退出 2；配置或调用失败退出 1；可能已经创建项目或会话时返回 `status: unknown` 并退出 3；确定中断且无未知持久结果时退出 130。若操作已完成但 stdout 写入失败，程序尽量在 stderr 输出 `output_delivery_failed` 和完整结果；这仍不授权自动重试。

`--timeout-secs` 默认 180，可设为 1 至 1800。项目探测与后续文本轮次共享调用预算；协议和节点收尾可能增加总时间。Ctrl-C 会请求取消并继续等待提供方返回最终状态；持久请求已派发时仍以 `unknown` 为准。错误不会自动重试、改派实例或降级项目选择。

该入口未提供持久调用结果库、跨进程崩溃恢复、会话续接或完整后代进程监督。需要保留 CLI 输出用于后续核验，不能把文本结果视为修复成功。

## 模拟启动入口

`crate::boot` 通过声明的 `recovery.simulation@1[local]` 服务装配通用框架和模拟任务引擎。任务调度、状态迁移与持久记录仍属于 recovery。

将 `CARGO_TARGET_DIR` 和 `RECUVORA_DATA_DIR` 设置为源码目录外的构建、运行目录，再执行：

```powershell
cargo run --locked -- demo --data-dir $env:RECUVORA_DATA_DIR
cargo run --locked -- demo --data-dir $env:RECUVORA_DATA_DIR --concurrency 1
cargo run --locked -- inspect --data-dir $env:RECUVORA_DATA_DIR --task <task-id>
```

`demo` 检查成功、明确失败、结果未知、验证失败、超时、授权拒绝、取消和故障后的新任务。每次使用新的任务 ID，数据保留供 `inspect` 查看，不自动删除。它只运行固定、无外部副作用的模拟任务，不接入 Harness、目标平台、远程节点、第三方插件或真实授权服务。

## 控制台入口

```sh
recuvora harness projects --config <harness-config> --extensions <extension-config> --harness harness-external --workspace work
recuvora harness run --config <harness-config> --extensions <extension-config> --harness harness-external --workspace work --visibility hidden --no-project --prompt "Reply with a short status"
recuvora serve --config <console-config>
```

`serve` 需要 `server` feature；同时启用 `web-ui` 时托管共享官方页面。服务端只接受回环监听；跨机访问使用 HTTPS 反向代理或 SSH 转发。调用需要独立令牌、准确来源和配置权限，不能把浏览器状态或插件输出当成授权。完整边界见[扩展协议](../../docs/extension-protocol.md)和[控制台 API](../../docs/console-api.md)。

开发规范见 [AGENTS.md](AGENTS.md)。
