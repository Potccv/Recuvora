# Recuvora

面向异构目标与工作负载的自愈式 AI Agent 宿主。系统持续接收提供方观测、维护故障事实，将获准任务交由指定 AI Harness 诊断与处理，在授权范围内验证目标状态，并将有效经验用于后续监控和处理。

**当前状态：已实现 Rust 通用框架、持久模拟任务引擎、Harness 与受限修复 CLI，以及接入可信 HTTP 服务的共享 Web/Tauri Desktop 官方控制台。** 页面包括监控与故障、宿主管理的插件监控专页、Harness、修复详情、审批、日志查询、模拟、能力和连接设置。运行时能力与审批动作由服务端决定，演示数据只在显式演示模式使用。外部节点与宿主侧契约插件已有受监督 stdio 接入，可通过配置的 SSH stdio 跨机连接；Harness 提供方统一由独立节点实现，宿主只保留通用 `remote-node` 代理。真实文件动作仍限 Windows 白名单既有 UTF-8 文件的有界全文替换；目标专属外部动作、完整业务恢复、直接 WebSocket Harness 适配及 macOS 节点运行尚未验证或实现。

代码实现与自动验证边界列在[实现状态表](docs/implementation-status.md)。通用监控与真实故障管理已有宿主实现；目标专属采集、活动部署、跨项目实机验收、自动诊断修复和完整业务恢复不属于宿主源码记录。

通用监控支持按可信模板自动发现并登记抽象目标：提供方新增合法目标键后无需逐项修改宿主监控列表。删除、改名与发现失败保留旧目标的未知状态及故障记录；插件不能通过发现结果改变调用白名单或规则。

## 当前可运行范围

- [通用框架](src/framework/README.md)：类型化服务、明确作用域和主版本、依赖校验、顺序启停、有界瞬时事件及实例资源清理。
- [通用监控与故障](docs/monitoring.md)：复用节点只读轮询，按配置比较观测，区分目标健康、时效和覆盖；检查点与故障原子持久化，支持归并、按新观测解除和带 revision 的人工确认。官方 UI 保留统一监控/故障中心，并为每个配置的插件提供通用专页；插件可登记受限声明式只读展示，但不能覆盖宿主状态或自动派发修复。
- [模拟任务引擎](src/recovery/README.md)：固定无副作用任务、有界队列与并发、同目标互斥、日志落盘与恢复；模拟授权不赋予真实操作权限。
- [CLI](src/boot/README.md)：`demo`/`inspect` 运行模拟演示与状态查询；`harness` 提供外部配置、实例与项目选择、独立项目创建和单轮文本入口；`repair` 提供执行、查询、人工批准/拒绝、撤销、应用及未知结果核验。[interfaces](src/interfaces/README.md) 负责 Harness 参数和 JSON 展示。
- [共享 UI](src/interfaces/ui/README.md)：Web 与 Desktop 共用独立认证页和官方控制台；首页围绕监控目标与故障，审批提供逐行差异，目标观测记录与按配置分类的错误记录独立实时更新并可暂停。持续有界读取按区域协调，保留输入焦点、草稿与展开状态；令牌只留页面内存，Unknown 不自动重试。
- [控制台服务](src/server/README.md)：Bearer 身份与权限、带 revision 的审批、持久操作回执、取消和按可信目标绑定的增量观测记录；显式诊断可持久关联来源故障，关联不授权或自动派发。关闭 UI 不停止宿主已接受的任务。
- [外部接入](src/nodes/README.md)：受监督 stdio、SSH stdio、版本化契约与能力白名单，供独立节点和宿主侧插件使用；配置存在不等于提供方可用。
- [Harness 接入](src/harnesses/README.md)：有界 JSON 配置、多个命名实例、显式默认/选择和节点工作区校验；通用 `remote-node` 代理支持 `Client`/`Hidden`、原生项目请求、客户端归组状态和显式注册的宿主工具路由。原生命令和提供方私有文件修改通道仍关闭。
- [审批与受限修复](docs/approval.md)：外部政策指定人工或 Harness 审批，宿主维护完整操作、范围、期限、一次执行许可及未知结果；[actions](src/actions/README.md) 对白名单中不超过 16 KiB 的既有 UTF-8 文件执行全文替换，并检查原内容和写后读回。
- [检查脚本](scripts/README.md) 和 [测试](tests/README.md)：格式、静态检查、框架与任务测试、真实测试宿主强杀后的恢复。

## 选择 Web 或 Desktop 前端

官方 UI 是项目统一维护的完整标准控制台，范围包括概览与管理、统一监控与故障中心、Harness 会话、修复详情、审批、日志查询和模拟实验室；这些权威页面不拆为需单独安装的插件业务页面。监控中心另列出每个配置的插件，并由宿主提供 `#/monitoring/plugins/{id}` 专页。插件可通过既有只读契约登记严格有界的字段布局；浏览器不连接插件，也不加载插件 HTML、JavaScript、CSS、Markdown、URL 或按钮。声明缺失、无效、调用失败或权限不足时，专页仍显示宿主拥有的连接、监控与发现数据。详细约束见[插件设计](docs/plugins.md#官方-ui-与插件监控专页)。

项目仍是一个 Cargo 包。Web 与 Desktop 不维护两套 UI：两者都读取 `src/interfaces/ui/public/`，页面内容和功能范围一致。`scripts/build-ui.ps1` 提供三个前端目标，构建目录必须位于项目根之外：

```powershell
./scripts/build-ui.ps1 -Target Web -BuildDir $env:RECUVORA_UI_BUILD_DIR
./scripts/build-ui.ps1 -Target Desktop -BuildDir $env:RECUVORA_UI_BUILD_DIR
./scripts/build-ui.ps1 -Target All -BuildDir $env:RECUVORA_UI_BUILD_DIR
```

- `Web` 生成可由部署方静态托管的目录；`server` 与 `web-ui` 同时启用时，也可由宿主同源托管共享资产。
- `Desktop` 构建 `recuvora-desktop` Tauri 可执行文件；当前不生成安装包。
- `All` 同时返回 Web 目录和 Desktop 可执行文件；它只表示两个前端目标，不额外构建默认 CLI。

共享静态 UI 为零第三方前端依赖，三个目标都需要已有 Node.js 进行语法检查和资产整理；Desktop/All 还需要 Rust 1.98.1、平台链接器及 Tauri 构建依赖。默认生成 Release Desktop，可用 `-Profile Debug` 调整。构建会在外部目录生成内容标识的 Web 资产、SHA-256 资产清单及 Cargo target；完整参数和输出结构见[脚本文档](scripts/README.md#共享-ui-构建)。

根 manifest 的 `cli`、`server`、`web-ui`、`desktop-ui` 和 `all-ui` feature 控制同一包内的编译组合；`desktop-ui` 隐含 `web-ui`。Harness 提供方不编入宿主，统一通过 `remote-node` 接入。Desktop 保持无业务 command 的薄壳，与 Web 通过同一 HTTP 客户端接入宿主。页面默认要求显式连接服务，功能由服务返回的权限、能力和依赖状态决定。

配置 [控制台服务](docs/console-api.md) 后运行 `cargo run --locked --features server,web-ui -- serve --config PATH`。服务仅绑定回环地址；跨机访问通过 TLS 反向代理或 SSH 转发，Desktop 可填写 HTTPS 服务地址或本机转发端口。节点进程另通过 SSH stdio 接入；这不代表已经验证 macOS 节点或任意目标平台。

需要 Rust 1.98.1 及平台链接器。先将 `CARGO_TARGET_DIR`、`RECUVORA_TEST_TEMP`、`RECUVORA_DATA_DIR` 分别设置为项目外的构建、测试和专用运行目录：

```powershell
./scripts/check.ps1 -BuildDir $env:CARGO_TARGET_DIR -TempRoot $env:RECUVORA_TEST_TEMP
cargo run --locked -- demo --data-dir $env:RECUVORA_DATA_DIR
```

模拟 CLI 不会调用命令、网络、Harness 或桌面；Harness library 尚未接入该演示入口。日志达到上限后拒绝新写入；未知目标保持阻断，需要后续核验机制处理。当前不承诺长期无人值守或任意恶意/阻塞代码的故障隔离。

Harness 使用独立命令路径。先把[外部节点配置示例](profiles/harnesses.remote.example.json)和[扩展连接示例](profiles/extensions.example.json)复制到项目外，安装并配置提供方节点，然后设置 `RECUVORA_HARNESS_CONFIG` 与 `RECUVORA_EXTENSIONS_CONFIG` 为活动配置路径：

```powershell
cargo run --locked -- harness list --config $env:RECUVORA_HARNESS_CONFIG
cargo run --locked -- harness projects --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --workspace work
cargo run --locked -- harness run --config $env:RECUVORA_HARNESS_CONFIG --extensions $env:RECUVORA_EXTENSIONS_CONFIG --workspace work --visibility hidden --no-project --prompt "仅回复：连接成功"
```

列表只检查本地装配条件，不探测节点内的外部服务状态；调用可用 `--harness` 选择实例。必须明确选择节点工作区、可见性和项目策略；Ctrl-C 会等待提供方收尾，派发后不确定的持久结果以 `unknown` 返回且不自动重试。完整参数、项目创建、输出和退出码见 [Harness 使用入口](src/boot/README.md#harness-使用入口)。

`repair` 使用另一份项目外 JSON 配置，分别指定 `execution_harness` 和 `policy.reviewer.harness_id`；二者可为同一实例。执行与审批会话均为 `Hidden`，审批使用独立工作目录且无工具，执行只开放明确注册的宿主工具。`repair run/inspect/approve/deny/revoke/apply/reconcile` 的配置和用法见 [审批与受限修复](docs/approval.md)；人工操作使用本地操作系统用户身份，不是远程身份认证接口。文件读回一致只证明本次文本替换，不代表目标业务已恢复。

## 运作方式

Recuvora 使用 Rust 可信宿主，通过服务契约、依赖管理、事件和插件生命周期装配业务。任务、授权、权威恢复记录与默认自愈流程作为可信内置模块共同编译发布；桌面原生操作、视觉及复杂外部依赖通过受监督的工作进程提供能力。

产品边界是宿主、独立宿主侧业务插件和外部节点。Recuvora 不提供节点发行构建；Harness、日志采集等节点由独立项目实现。首版宿主已提供受监督传输、握手、契约注册、受控调用入口和受限声明式插件监控专页，具体能力由独立提供方实现；插件登记的命名空间/schema 不授予文件写入或管理权限。宿主侧表示逻辑角色，独立插件默认进程外。安装包管理、任意前端代码或原生代码动态加载与完整新领域业务仍需各自实现，细则见 [插件设计](docs/plugins.md)。

| 实现类型 | 装配与更新方式 | 运行边界 |
| --- | --- | --- |
| 内置业务模块 | 从随宿主发布的实现中按配置选择；新增代码需要更新宿主 | 共享宿主进程与信任范围 |
| Harness 适配器 | 可信宿主注册已编入的通用 `remote-node` 代理；配置选择实例、节点地址和工作区 ID | 提供方协议与进程由独立节点管理；明确注册的工具经可信宿主处理，审批会话无工具 |
| 宿主侧契约插件 | 独立发布，配置结构化进程或 SSH 连接并注册版本化契约；可选登记声明式监控展示，当前不提供安装包管理 | 进程外，通过允许的契约与方法调用，不自动获得业务写权限；专页由宿主渲染，不加载插件前端代码 |
| 外部节点 | 独立项目按能力构建发布，通过受监督 stdio 或 SSH stdio 接入 | 节点自行管理本机依赖与工作进程；宿主保留断连、超时、取消和未知结果 |

独立发行插件也可作为隔离能力提供方；包的发布方式和进程划分是不同维度。多个可信能力可以共享一个工作进程，不要求每项服务单独进程。第一方系统实现优先 Rust，Python 仅用于确有依赖需要的外部能力。

通用框架不包含目标专属规则、模型提供方私有业务或默认修复流程。业务消费者依赖服务契约，装配层选择实现；同一适配器可配置多个 Harness 实例，调用显式选择实例且失败时不静默改派。配置地址只由已注册适配器解释，不会下载 SDK 或把地址当作任意代码入口。同进程调用不强制经过网络编码，跨进程调用保留延迟、取消和结果未知的真实语义。首版不以任意 Rust 原生动态库热加载作为扩展机制。

## 项目结构

```text
.
├── README.md
├── AGENTS.md
├── .gitignore
├── .github/workflows/          # Windows 自动检查，生成物与测试数据位于源码外
├── src/
│   ├── README.md              # 能力模块导航
│   ├── AGENTS.md              # 模块开发通用规范
│   ├── lib.rs                 # 导出已实现能力；server 按 feature 编译
│   ├── main.rs                # 调用 boot 的程序入口
│   ├── bin/
│   │   └── recuvora-desktop.rs # feature 限定的 Tauri 薄入口
│   ├── framework/             # 上下文、服务、依赖、事件与生命周期
│   ├── boot/                  # 启动、配置及组合校验
│   ├── server/                # 可选可信 HTTP 控制台服务与持久操作回执
│   ├── recovery/              # 任务、授权、持久记录与自愈流程
│   ├── nodes/                 # 宿主侧节点连接、能力路由与事实核对
│   ├── monitors/              # 通用只读监控、规则判定、时效与覆盖
│   ├── harnesses/             # 多实例 registry、远端代理、独立审批角色与宿主工具路由
│   ├── actions/               # 受限文本替换；其他目标操作与业务验证规划
│   ├── platforms/
│   │   ├── linux/
│   │   ├── windows/
│   │   └── macos/
│   ├── vision/                # 视觉识别与推理
│   ├── knowledge/             # 经验存储、检索与沉淀
│   ├── interfaces/            # CLI 参数、共享官方 Web/Desktop 控制台及 API 客户端
│   │   └── ui/public/         # 两个前端共用的静态 UI 源
│   ├── notifications/         # 告警与通知渠道
│   ├── protocol/              # 跨进程、跨节点的消息与代理映射
│   └── sdk/                   # 本地与远程扩展开发接口
├── profiles/                  # 部署组合规划、Harness 与受限修复配置示例
├── docs/                      # 共享设计与文档导航
├── scripts/                   # CLI 检查及 Web/Desktop/All 前端构建入口
├── tests/                     # 集中测试与测试进程故障注入
├── Cargo.toml                 # 单个 recuvora 包、feature、二进制、依赖和测试目标
├── Cargo.lock                 # 可复现依赖
├── build.rs                   # desktop-ui 的 Tauri 构建输入与 schema 在项目外暂存
├── tauri.conf.json            # Desktop 壳窗口及共享资产入口
└── rust-toolchain.toml         # 固定开发工具链
```

项目采用单个 Cargo 包 `recuvora`，包含库、默认同名 CLI 和 feature 限定的 `recuvora-desktop`。`src/lib.rs` 导出通用框架、恢复业务、动作、Harness、节点、协议、交互及启动模块，`server` 按 feature 编译；`src/main.rs` 按命令启动所需服务。Desktop 二进制打开共享 UI 并通过同一 API 访问宿主。已实现模块使用各自 `mod.rs`，只有规划文档的能力不代表已经可运行。

各能力目录及平台子目录分别维护 README.md 与 AGENTS.md。职责边界由模块、可见性和服务契约维护；模块不自动等同于插件或进程。依赖和集成测试目标统一在根 `Cargo.toml` 登记，私有模块测试也从集中 `tests/` 引用，故障注入通过集成测试自身的子进程入口完成。项目目录只保留计划提交并推送到 GitHub 的源码、项目文档和必要配置；部署模板、安装包和运行数据分别管理，测试临时文件、构建产物、缓存、用户配置、数据库、日志和录屏不放进源码目录。

单 Cargo 包要求只约束宿主源码；外部节点和第三方插件可以采用自己的仓库、语言和构建体系，不依赖整个宿主 crate。Harness 通过通用外部节点契约接入；可信文件动作与权威审批仍由宿主负责，不因 Harness 远程化而开放节点任意写入。

## 导航

- 组合与业务：[框架](src/framework/README.md)、[启动](src/boot/README.md)、[自愈服务](src/recovery/README.md)、[节点](src/nodes/README.md)、[部署组合](profiles/README.md)。
- 其他能力及开发接口：[模块导航](src/README.md)。
- 共同设计：[架构](docs/architecture.md)、[审批与受限修复](docs/approval.md)、[插件契约](docs/plugins.md)、[文档索引](docs/README.md)、[实现状态](docs/implementation-status.md)、[测试记录](tests/README.md)。
- 开发约定：[项目 AGENTS](AGENTS.md)、[模块 AGENTS](src/AGENTS.md)。

## 自愈闭环

以下是完整系统的目标流程；当前受限修复从显式 CLI/API 请求开始，尚未实现自动监控触发、业务健康核验或知识沉淀。

1. 监控提供方采集故障和环境证据，保留来源、时间与稳定事件标识；失联或过期观察标记为未知。
2. 任务服务记录事件，默认自愈流程通过知识服务检索经验，并调用指定 Harness 生成诊断、计划和验证方法。
3. 已知方案仅在适用条件与既有授权均满足时自动执行；需要新审批时由政策选择人工或受委托的 Harness。超出委托范围、证据不足或审批不可用时等待人工，不自动扩大已有授权。
4. 授权服务确认范围，执行端再次检查目标、令牌和允许操作，持久记录意图后执行。
5. 验证目标业务结果；未确认的外部副作用先核验，失败时停止、按支持情况回退或交给人工。
6. 权威记录保存结果，知识服务沉淀候选经验及验证证据。知识暂不可用时保存待交付记录，不重新执行已完成修复。

任务、授权及持久记录服务是运行必需能力；由配置明确权威提供者，初始化和恢复完成后才开放执行。替换默认流程不允许绕过这些服务。审批 Harness 的结构化评估由可信授权服务核验和持久化，普通模型文本、知识建议或界面声明不直接产生执行许可。

## 目标变更适配

目标版本或环境变化导致既有流程失效时，提供方保存版本、错误和有界证据。缺少已授权适用方案时提交政策指定的审批入口；当前受限文本修复不开放目标专属外部动作，此类动作仍需独立执行契约与明确授权。

AI 仅在获准目标的隔离副本或分支内调整代码、配置及其他目标资产，核验后按授权发布并保留回退版本。动作接口成功不代表任务成功，必须通过独立提供方观测验证目标业务状态。

运行时修复 AI 只修改获准目标与临时副本，不修改 Recuvora、插件包、启用配置或自身更新机制。目标专属规则、资产和验证逻辑由对应集成项目维护。

## 长期运行与跨平台

以下是运行设计要求；系统服务部署、其他平台与长期资源趋势仍需单独实现或验收，详见[状态表](docs/implementation-status.md)。

- 进程隔离限制外部能力的故障影响；同进程可信模块共享故障与权限范围，不能靠插件接口当作安全沙箱。
- 持久化任务、授权、执行意图和结果；断连或超时不证明操作未发生，不可安全重复的外部动作不能盲目重放。
- 重试、队列、缓存、日志、并发和工作进程资源均有界；卸载清理订阅与资源，不自动撤销已发生的外部动作。
- 系统服务监督负责宿主退出后的恢复；宿主监督自己启动的插件工作进程，外部节点监督其本机工作进程，旧执行者失去操作能力后才转移资源锁。
- Linux 分别验证 X11、Wayland 与所选后端；Windows 区分后台服务和交互会话；macOS 检查采集与辅助功能权限。
- 支持范围绑定实测系统版本、CPU 架构、后端和会话。平台或视觉能力不可用时暂停受影响任务，未受影响能力继续运行。

## 实现顺序与待定事项

当前已接通多 Harness registry、独立审批角色、持久授权、Windows 受限文本动作、官方控制台 HTTP API 及外部节点契约。后续重点是完整任务调度与业务核验、其他本机平台和目标环境，以及更完整的插件生命周期；直接 HTTP/WebSocket Harness 适配仍未实现。受限文件替换、协议夹具或构建检查不能代替目标专属集成和长期运行验证。

模拟任务与审批分别使用自己的版本化持久记录和单写入者锁，审批记录绑定政策、完整操作、有效期、执行意图及未知结果，详见 [审批说明](docs/approval.md)。当前跨进程协议已实现 JSONL v1；生产存储迁移、插件发行包格式、平台后端及长期可靠性仍待验证，不承诺任意插件热更新。

Rust 源码采用 [Cargo 标准包布局](https://doc.rust-lang.org/cargo/guide/project-layout.html)。
