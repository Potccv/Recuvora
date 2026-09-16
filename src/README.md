# Recuvora 源码模块

项目使用根目录唯一的 Cargo package，在 `src/` 内按能力组织 Rust 模块、服务契约和领域消费者。框架、业务与装配保持独立职责，进程位置由部署和故障边界决定。

**当前状态：已实现 framework、recovery、monitors、boot、actions、Harness registry、CLI、可选可信 HTTP 服务与共享 Web/Tauri Desktop 官方控制台。** [lib.rs](lib.rs) 声明已有模块，薄入口 [main.rs](main.rs) 按命令启动模拟、Harness、受限修复或控制台服务；Desktop 只承载同一 UI。节点与协议模块支持受监督 stdio/SSH stdio 外部接入。执行与审批保持独立 Hidden 会话，真实动作仍限 Windows 白名单文本替换。UI 默认先显示独立认证页，通过宿主认证后显示运行壳；演示需显式切换。其他目标专属集成、macOS 节点部署、直接网络 Harness 适配器及完整业务恢复未完成验证。

## 能力导航

下表是职责导航，不表示每个能力已经有可运行实现。代码与自动验证边界统一见[实现状态表](../docs/implementation-status.md)；`platforms`、`vision`、`knowledge`、`notifications` 和 `sdk` 目前以规划文档为主。

| 模块目录 | 维护职责 |
| --- | --- |
| [framework](framework/README.md) | 通用上下文、服务注册、依赖、事件、实例与资源生命周期 |
| [boot](boot/README.md) | CLI 与 Tauri 启动、共享宿主装配及配置校验；升级机制仍待实现 |
| [server](server/README.md) | 版本化 HTTP API、认证与权限、操作回执、宿主日志与目标观测记录、故障来源关联和可选同源共享 UI |
| [recovery](recovery/README.md) | 封闭模拟任务、持久真实故障、独立审批与受限修复；完整自愈调度规划 |
| [nodes](nodes/README.md) | 宿主侧节点身份、连接、能力路由与执行事实核对 |
| [monitors](monitors/README.md) | 通用只读轮询、配置规则、时效与覆盖检查、持久检查点及故障投递 |
| [harnesses](harnesses/README.md) | 多 Harness 实例、远端工作区、会话与项目状态、独立审批角色及受控宿主工具路由 |
| [actions](actions/README.md) | Windows 白名单文本读取与全文替换；发布、回退及业务核验规划 |
| [platforms](platforms/README.md) | 桌面能力契约、消费与各平台节点接入约束 |
| [vision](vision/README.md) | 画面识别、OCR 与视觉推理规划 |
| [knowledge](knowledge/README.md) | 经验存储、检索、适用条件与候选沉淀规划 |
| [interfaces](interfaces/README.md) | CLI 参数与 JSON 结果；Web/Desktop 共用认证入口、官方页面、目标观测记录流、API 客户端和安全文本展示 |
| [notifications](notifications/README.md) | 通知与投递结果规划 |
| [protocol](protocol/README.md) | 跨进程、跨节点消息、握手及服务代理映射 |
| [sdk](sdk/README.md) | 可选多语言开发辅助规划；已有 JSONL 协议不依赖 SDK |

## 模块与运行方式

宿主内置能力使用普通 Rust 模块；已有模块以 `mod.rs` 为入口，后续宿主实现也在所属能力目录内组织，不为每项能力、服务或 trait 创建独立 Cargo package。平台子目录按系统维护接入约束。模块目录维护 README.md 与 AGENTS.md，由父 README 导航；只有设计的能力无需创建空源码入口。

上述单包规则只适用于宿主实现。节点程序和第三方宿主侧插件由独立项目构建发布，新增业务契约/schema 和消费逻辑可以由插件提供；本目录不收纳节点源码或节点构建目标。尚未实现的平台与采集目录维护宿主契约、消费、代理及接入约束，具体节点后端在外部项目中实现。通用机制与授权边界见 [插件设计](../docs/plugins.md)。

项目根 `Cargo.toml` 统一维护产品、依赖、构建与测试目标，`src/` 下不放置子 manifest。全部测试代码集中在 [tests](../tests/README.md)，开发命令见[项目说明](../README.md)和[脚本说明](../scripts/README.md)。

Web 与 Desktop 页面只维护在 `interfaces/ui/public/`。Web 构建复制为项目外静态目录，也可由 `server` 同源托管；Desktop 嵌入同一资产并连接同一 HTTP 服务。`cli`、`server`、`web-ui`、`desktop-ui` 和 `all-ui` 是同一包内的编译选择，不形成 workspace 成员；服务权限与提供方状态决定实际能力。前端构建见[脚本说明](../scripts/README.md)，认证与部署见[控制台 API](../docs/console-api.md)。

共享 UI 在认证后持续串行读取有界快照，协调更新页面区域并保留编辑控件、焦点与页面内草稿。目标提供方观测记录在独立页面按游标增量读取，普通记录与按配置分类的错误记录双栏展示；记录方法和目标参数来自可信宿主配置，页面不能选择任意资源或提供方。故障可由操作员显式关联一次受限修复，来源关联不授予审批或证明业务恢复。

内置模块随宿主共同编译发布，按配置选择已经存在的实现；独立安装扩展默认进程外，需经元数据校验、实例握手和能力注册。Harness registry 中的 `adapter` 只选择宿主已注册的适配器，`address` 由该适配器解释，不会通过配置下载或加载 SDK。复杂平台、视觉和外部运行时采用隔离工作进程，也可以代理形式参与同一依赖与事件体系。

能力模块、插件实例和工作进程并非一一对应。契约不必对应运行插件，装配模块可以选择多个提供方；同进程模块与跨进程代理均不能绕过授权或隐藏未知结果。

## 装配与开发入口

[部署组合](../profiles/README.md) 说明宿主组合、插件与外部节点绑定，并提供当前本地 Harness 和受限修复 JSON 示例；不构建或发布节点。`harness` 与 `repair` 各自显式加载外部活动配置，模拟命令不使用它们。执行实例和政策中的审批实例可相同，具体契约及 CLI 流程见 [审批与受限修复](../docs/approval.md)。实际启用配置、安装包、数据和日志不在源码目录中维护。

遵守 [本层 AGENTS](AGENTS.md) 及 [项目 AGENTS](../AGENTS.md)。全局运行关系见 [架构](../docs/architecture.md)，服务与加载契约见 [插件设计](../docs/plugins.md)。
