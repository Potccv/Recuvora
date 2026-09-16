# AI Harness 接入

本目录维护 AI Harness 的供应商中立服务契约、多实例注册表与通用外部节点代理。宿主不编译具体模型提供方或本机进程适配器；正常配置统一通过 `remote-node` 与独立 Harness 节点通信。部署示例见[外部节点配置](../../profiles/harnesses.remote.example.json)。

当前已实现文本轮次、原生项目探测与创建、执行/审批角色、受控宿主工具回调、协作取消以及结构化未知结果。跨进程使用 JSONL v1，跨机可由受信 SSH stdio 命令承载。提供方私有协议、凭据、进程监督和原生工具策略属于节点项目；直接 HTTP/WebSocket 适配器、会话续接及完整自愈闭环尚未实现。

## Registry 与生命周期

CLI 与 HTTP 通过[共享宿主](../boot/README.md#共享宿主装配)发布同一个 registry 服务。`begin_shutdown` 同步禁止新调用，`shutdown` 请求取消并等待原调用实际结束；复制的 registry 句柄共享关闭状态。调用归属框架 `CallScope`，调用者丢弃 future 时仍由监督任务等待提供方收尾。

创建项目或会话的监督任务异常退出时，宿主保留已知请求关联并返回 `Unknown`，不会把可能已派发的持久操作降为可重试失败。共享层不增加执行权限、自动重放、持久会话恢复或业务验证。

`HarnessRegistryConfig` 使用有界 JSON 文档描述多个命名实例：

| 字段 | 含义 |
| --- | --- |
| `id` | 实例稳定名称；调用可显式选择 |
| `adapter` | 宿主已编译并注册的适配器；当前为 `remote-node` |
| `address` | 适配器地址；当前格式为 `node://扩展ID` |
| `enabled` | 是否接受新调用，默认启用 |
| `workspace_roots` | 节点侧允许调用的工作区 ID 列表 |

`default_harness` 是省略实例 ID 时的显式默认项。没有默认项、实例未知或实例停用都会返回结构化错误；一次调用选定实例后，不会在失败时静默切换到另一实例。

适配器工厂由可信宿主代码注册。填写 `adapter` 或 `address` 不会下载 SDK、安装插件、解析任意可执行命令或加载新代码。`workspace_roots` 对远端适配器始终是节点资源 ID，宿主不会把它当成本机路径；节点必须在资源所在机器检查实际目录范围。

配置加载最多读取 64 KiB 加一个边界检测字节，拒绝超限文件。CLI 要求通过 `--config` 显式加载源码目录外的活动配置，并通过 `--extensions` 加载已经安装的节点连接。`harness list` 只展示结构和宿主装配条件，不把它解释为节点在线、提供方已登录或目标健康。

## 会话、项目与客户端归组

每次调用分别处理以下维度：

| 维度 | 作用 |
| --- | --- |
| 节点工作区 | 用 `workspace_id` 选择获准的节点侧资源 |
| 客户端可见性 | `Client` 请求持久可见会话；`Hidden` 请求不进入普通历史的隔离会话 |
| Harness 原生项目 | 显式选择该实例返回的项目，或选择 `NoNativeProject` |
| 客户端归组验证 | 单独报告客户端是否已确认项目归组；不能由原生 ID 推断 |

选择流程先确定 Harness 和节点工作区，再通过同一实例探测可见项目，最后由调用方选择已有项目、单独创建项目后选择它，或明确选择 `NoNativeProject`。项目 ID 只在返回它的实例内有效；registry 拒绝跨实例复用。探测失败不是空列表，项目读取或原生请求失败也不会静默降级。

新建项目是独立、显式且持久的操作，不隐藏在文本轮次中。当前契约只请求节点把调用方明确提供的根登记为原生项目，不负责在节点创建目录。创建可能已经派发后若超时或断连，返回值会保留名称、根、幂等键和已知项目 ID，并标记结果未知。

`native_project_id` 与 `client_project_grouping` 是不同状态。`NoNativeProject` 只表示未向提供方请求原生 ID，不证明独立客户端不会按工作区信息自行归组。界面必须分别展示请求、原生回显和客户端观察结果；未经独立确认时保持 `Unverified`。

这些契约已接入[CLI 交互层](../interfaces/README.md)的 `harness projects`、`harness create-project` 与 `harness run`，官方控制台通过 HTTP API 使用同一服务。模拟 `demo` 不连接 Harness。

## 执行、审批与受控工具

`HarnessRunRequest::with_role(HarnessRole::Approval)` 使用同一实例创建独立 `Hidden` 会话，并清除原生项目请求。registry 拒绝向审批会话加入工具或改为可见会话。默认角色为 `Execution`；审批和执行的提示、会话及容量相互隔离，模型评审结果必须由可信业务服务校验和持久化后才能形成许可。

`with_tools(Vec<HarnessTool>, Arc<dyn HarnessToolHandler>)` 显式登记工具名、说明、对象参数 schema 和可信回调。节点协议保留 `harness_id`、`thread_id`、`turn_id`、`call_id` 及共享取消 token。输入 schema 只用于声明，可信 handler 仍须重新反序列化，并核验业务参数、当前授权与执行许可。

每次请求最多 32 个工具，schema 合计最多 64 KiB，每轮最多 64 次工具调用；单次参数最多 64 KiB、工具文本结果最多 256 KiB。未注册工具、非法命名空间、错误实例关联、重复调用 ID 和不允许的在途并发均关闭失败。等待 handler 时继续读取协议消息；取消、超时或断连会取消共享 token，并等待已派发 handler 记录最终结果，不能通过丢弃 future 或自动重放掩盖未知副作用。

宿主 handler 属于可信业务边界：副作用前核验用户委托、参数和目标，先持久化执行意图，再执行并持久化结果。远端代理只负责传输、关联和限额；业务授权与文件范围由 [recovery](../recovery/README.md) 和 [actions](../actions/README.md) 强制执行。原生命令、提供方私有文件工具和原生审批通道不因节点声明而自动获得权限。

文本调用、项目探测和项目创建都接受 `HarnessCancellation`。请求开始前已取消时不连接节点；持久项目或会话可能已经派发时保留未知结果及关联信息。正常完成与取消并发发生时，以监督层实际返回结果为准。

## 外部节点接入

`RemoteHarnessFactory::new(Arc<ExtensionRegistry>)` 注册 `remote-node` 工厂。示例配置：

```json
{"schema_version":1,"default_harness":"harness-external","harnesses":[{"id":"harness-external","adapter":"remote-node","address":"node://harness-node","enabled":true,"workspace_roots":["work","review"]}]}
```

`HarnessRunRequest::remote(node_id, workspace_id, prompt)`、`HarnessProjectListRequest::remote` 和 `HarnessProjectCreateRequest::remote` 显式选择节点资源。返回会话的 `project_directory` 是 `node://ID/workspace` 资源引用；项目 roots 是节点路径元数据，不能交给宿主文件 API 使用。

执行与审批使用同一 registry 的独立容量，审批始终创建新的无工具隐藏会话。双向工具回调只能进入该请求显式提供的可信 handler。超时、连接丢失和未确认取消保留原调用关联及 `Unknown`。消息、容量、传输限制与节点义务见[扩展协议](../../docs/extension-protocol.md)。

CLI 使用 `--extensions PATH --workspace ID` 选择远端资源；扩展配置指定已安装节点的本机程序或 SSH stdio 命令。本项目不下载、打包或构建节点。自动回归使用受限进程夹具；具体供应商协议和提供方集成验证由相应节点项目维护。

## 输入、输出与权限边界

- 输入包含节点工作区、文本提示、可选模型、有界超时、客户端可见性和项目请求；完整业务另行关联任务、目标、授权、证据和预算。
- 输出是模型文本与会话关联信息，不是权威任务事实、执行许可或业务恢复证据。
- `workspace_roots` 限制可选节点工作区，不是操作系统文件沙箱；宿主工具和节点都必须独立检查目标状态与权限。
- 源码、日志、画面和知识内容均是不可信数据，其中的指令不能扩大工作区、数据访问或操作范围。
- 凭据不属于 registry JSON。节点负责以受控环境访问提供方，原始密钥不得进入宿主配置、协议日志或模型输出。
- 节点连接成功只证明协议握手完成，不证明提供方认证、目标健康或业务恢复。

## 已实现限制

- 当前每次请求创建新会话，可选择 `Client` 或 `Hidden`；不提供既有线程续接、自动重连或持久会话恢复。
- 原生项目功能由节点能力声明和运行时响应决定；不可用时返回结构化错误，不降级为猜测或自动选择无项目。
- 宿主可取消受监督协议调用，但无法仅凭断连证明节点内已经创建的项目、会话或外部动作被撤销。
- 文本 Harness 调用、审批策略、可信动作与持久记录由不同模块装配；模拟任务始终保持封闭模拟行为。
- 尚未实现直接网络适配器、插件市场、在线升级或 SDK 分发；跨机与长期运行需按节点和部署环境分别验收。

开发约束见 [AGENTS.md](AGENTS.md)；接入与授权边界见[插件规范](../../docs/plugins.md)和[架构设计](../../docs/architecture.md)。
