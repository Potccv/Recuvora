# 部署组合模板

本目录保存可审查的宿主部署组合示例。节点是独立构建和发布的外部产品；这些模板只描述宿主如何连接节点、选择 Harness 实例及装配审批修复，不承担节点构建、安装或启动角色。

当前可解析示例包括：

- [外部 Harness 节点](harnesses.remote.example.json)：注册 `harness-external`，通过 `remote-node` 连接 `node://harness-node`，并只允许节点侧 `work`、`review` 两个工作区。
- [外部扩展连接](extensions.example.json)：以 `kind: node` 启动 `harness-node` 占位程序，并仅允许 `recuvora.harness` v1 的三个已实现方法。
- [审批修复](repair.local.example.json)：执行与审批都选择 `harness-external`，同时限制目标、动作、文件和委托期限。
- [监控](monitors.example.json)：调用已安装扩展的只读方法，并用宿主规则判断观测与维护故障。

示例只使用非敏感占位值。使用时把副本放到 Recuvora 源码目录之外，按实际安装位置修改扩展程序、参数和工作目录，并保证节点 ID、Harness 地址及节点侧工作区 ID 一致。节点配置、模型上下文、运行数据和可修复目标应彼此隔离。SSH 部署的可信命令形式见[扩展协议](../docs/extension-protocol.md)。

## Harness 配置

Harness registry schema v1 使用以下字段：

| 字段 | 作用 |
| --- | --- |
| `schema_version` | 配置版本，当前为 `1` |
| `default_harness` | 调用未指定实例时使用的已启用 Harness ID |
| `harnesses` | 一个或多个命名 Harness 实例 |
| `adapter` | 选择宿主已注册的适配器；当前外部节点使用 `remote-node` |
| `address` | 适配器地址；`remote-node` 使用 `node://ID` |
| `workspace_roots` | 节点侧获准工作区 ID，不是宿主本机文件路径 |

客户端可见性和原生项目请求属于每次调用，不写入部署模板。应用应先选择 Harness，再向同一实例探测项目，并让用户选择已有项目、新建项目或明确选择不关联项目。项目 ID 只在返回它的 Harness 实例内有效，不能跨实例复用或固化到可提交模板。原生项目状态和独立客户端的归组状态分别报告，不能由配置相互推断。

项目或会话创建派发后若连接失败，宿主保留结构化未知结果；调用方必须先核验外部状态，不能自动重试。地址字段不会下载 SDK、安装程序或解释任意 shell 命令。直接网络 Harness 适配器尚未实现，不能通过把地址改成 `http` 或 WebSocket URL 来启用。

CLI 不自动发现源码示例，并拒绝直接把源码内文件作为活动配置。将外部副本传给 `harness list --config <path>` 时，列表会校验结构并展示本地装配条件，但不探测外部登录状态；其他命令只装配明确选定的实例，失败不会自动改派。

## 审批修复配置

`repair.local.example.json` 通过 `harness_config` 引用独立 registry，通过 `extensions_config` 连接节点。执行实例与 `policy.reviewer.harness_id` 可以相同，但模型执行与审批必须使用不同会话，并分别选择 `work`、`review` 节点工作区；宿主文件目标仍由 `target_root` 单独限制。示例要求预先存在 `target-copy/settings.txt`，状态写入 `repair-state`；`review-context` 只作为本地人工接管占位，使用远端审批工作区时不作为模型目录。复制到源码外后按实际位置调整。

`policy` 是操作员明确提供的审批委托，包含目标、动作、文字、有效期和版本硬范围。普通实例启用不能代替委托。`allowed_files` 约束实际读写；当前只支持每个不超过 16 KiB 的既有 UTF-8 文件。人工审批不调用模型，Harness 审批使用独立的无工具会话。配置与恢复规则见[审批说明](../docs/approval.md)。协议夹具已覆盖宿主工具、隔离文件、远端调用和未知结果边界；这些结论不等于完整业务恢复。

## 监控配置

`monitors.example.json` 描述宿主如何调用已安装扩展的只读方法并判断观测。`monitor-001`、`target-001`、`source-001`、`observation-extension` 和 `com.example.observation` 都是无业务含义的占位值，本项目不提供对应实现。契约插件需先登记相应 schema，提供方返回[监控批次](../docs/monitoring.md)。活动副本由控制台配置的 `monitors_config` 引用；权限按需授予 `monitor.read`、`incident.read` 和 `incident.acknowledge`。人工确认只记录关注，不会授权修复。

## 组合边界

| 组合 | 用途 |
| --- | --- |
| 同机节点绑定 | 宿主连接同机独立部署的节点，按能力与工作区选择提供方 |
| 远程节点绑定 | 宿主通过受认证连接使用目标环境节点，仍保留任务、授权和权威恢复记录 |
| 第三方业务扩展 | 独立插件注册业务契约与消费逻辑，再绑定实现相应契约的节点 |
| 桌面能力绑定 | 宿主绑定运行在获准交互会话中的节点，消费观察、输入与验证能力 |

这些是宿主组合，不是本项目发布的节点程序或构建变体。同机与远程只改变连接位置，节点仍有独立产品、进程和故障边界。宿主只监督自己启动的插件工作进程；节点自行监督其本机进程，连接断开不能证明远端已经停止或回收。

内置实现只能从随宿主编译发布的代码中选择。独立安装扩展默认进程外；当前支持受限只读第三方契约注册和消费，但尚无自动下载、安装市场或在线热升级。安装、启用、能力权限与任务授权分别管理；契约或 schema 注册不产生权限，也不能替代可信授权和权威记录。共同规则见[插件设计](../docs/plugins.md)。

## 验证清单

- 验证角色组合、必需依赖、作用域与提供者冲突，避免双重授权或恢复记录分叉。
- 验证配置变更失败、节点不可用和关键服务缺失时相关执行关闭。
- 验证多个 Harness ID 的显式选择、停用与默认项，以及实例失败时不隐式改派。
- 验证工作区越界拒绝、跨实例项目 ID 拒绝和项目探测失败不冒充空列表。
- 验证可见性与项目选择组合，并保持未经独立观察的客户端归组为 `Unverified`。
- 验证插件契约命名空间与版本冲突、缺失消费插件及远端能力不可用时的装配阻断。

装配机制见[插件设计](../docs/plugins.md)，部署边界见[架构](../docs/architecture.md)，维护规范见 [AGENTS.md](AGENTS.md)。
