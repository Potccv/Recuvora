# 外部扩展协议 v1

本协议连接独立发布的节点与宿主侧业务插件。Recuvora 不包含节点程序或节点构建角色。当前实现支持可信配置声明的外部程序、版本化握手、业务契约注册、受限调用和双向回调；不是任意原生动态库加载器，也不是操作系统权限沙箱。

## 传输与进程

采用 UTF-8 JSONL，每行一个完整对象，`type` 区分消息。单帧含换行最多 1 MiB；未知消息、未知字段、错误关联和重复回调关闭失败。stdout 仅用于协议，诊断写 stderr。宿主首版丢弃扩展 stderr，避免无限日志或凭据回显；节点自行维护有界诊断。

每次探测或顶层调用启动一个配置指定的子进程。探测在握手完成后关闭 stdin；业务连接在握手后只允许一个顶层调用，期间可双向回调。正常最终回执后关闭 stdin，最多等待 10 秒排空。取消或截止时间到达时发送 `cancel`，最多等 10 秒终态；在途可信工具处理器仍须等待其持久结果。宿主最终执行 kill/wait 回收直接传输子进程，不因此声称远端执行者或任意后代均已停止。

本机可以直接启动独立节点/插件；跨机器使用受信任 SSH 程序及固定参数承载 stdio。SSH 负责服务器身份校验、认证与加密，必须在部署时验证主机密钥并使用非交互认证。协议中的 `expected_id` 只验证配置绑定，不代替 SSH 身份认证。不实现裸 TCP、HTTP 或 WebSocket 监听，也不自动拼接 shell 命令；配置中的远程命令由部署者审查，用户提示、工作区和工具参数不参与命令拼接。

## 可信配置

`ExtensionsConfig` 使用 schema_version 1，最多 64 个扩展、配置最多 256 KiB。下面为结构示例，路径和命令须按独立项目实际安装调整：

```json
{
  "schema_version": 1,
  "extensions": [
    {
      "id": "harness-node",
      "kind": "node",
      "enabled": true,
      "command": {
        "program": "/usr/bin/ssh",
        "args": ["-T", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "harness-machine", "configured-node-command"],
        "cwd": "/srv/recuvora",
        "env": {}
      },
      "namespaces": [],
      "allow_calls": [
        {"contract": "recuvora.harness", "version": 1, "method": "run"},
        {"contract": "recuvora.harness", "version": 1, "method": "projects"},
        {"contract": "recuvora.harness", "version": 1, "method": "create_project"}
      ],
      "allow_nodes": []
    }
  ]
}
```

`program` 和 `cwd` 是宿主机器的真实路径。相对路径以配置文件目录为基准；程序必须存在，工作目录必须存在。参数最多 64 个、每个最多 8 KiB；显式环境覆盖最多 32 项。外部程序以宿主当前身份和继承环境运行，配置属于本机代码执行信任边界；应通过操作系统用户、目录权限及部署隔离限制程序权限，不在配置中保存原始密钥。运行配置保存在源码之外。

`kind` 为 `node` 或 `plugin`。`namespaces` 只允许插件声明，不能占用 `recuvora` 保留域，不能与其他插件的命名空间重叠。`allow_calls` 明确允许调用的契约、版本和方法；声明能力不自动授予调用权。`allow_nodes` 只允许插件消费列出的节点只读方法。

## 握手与声明

宿主首先发送：

```json
{"type":"hello","protocol_version":1,"expected_id":"harness-node","kind":"node"}
```

扩展在 5 秒内回答：

```json
{
  "type": "ready",
  "protocol_version": 1,
  "id": "harness-node",
  "kind": "node",
  "contracts": [
    {
      "id": "recuvora.harness",
      "version": 1,
      "methods": [
        {"name":"run","read_only":false,"input_schema":{"type":"object"},"output_schema":{"type":"object"}}
      ]
    }
  ],
  "capabilities": ["text", "client_visibility", "approval", "tools"],
  "workspaces": ["work", "review"]
}
```

身份、角色和协议版本必须精确匹配。每个扩展最多 32 个契约、每契约 32 个方法，能力与工作区各最多 64 个。重复或非法标识拒绝注册。每次业务调用的握手必须与登记时声明完全一致（包括列表顺序）；升级声明需显式重建 registry，不能在运行期间静默替换。

插件先注册其获准命名空间中的新契约及 schema，节点随后只能实现已注册的相同契约版本和声明。内置 `recuvora.harness` v1 由宿主已有消费者处理。单个扩展握手失败、消费者缺失或声明不兼容只停用相关实例，`ExtensionRegistry::statuses()` 返回原因；配置本身非法则整体拒绝。注册成功仅表明连接和声明有效，不证明模型已登录或目标健康。

## 调用、回调与结果

```json
{"type":"call","id":"unique-call-id","contract":"com.example.records","version":1,"method":"query","params":{"target_key":"target-001"},"timeout_ms":30000}
{"type":"result","id":"unique-call-id","result":{"entries":[]}}
```

`timeout_ms` 为相对毫秒期限，最大 1800000；不要求两端墙上时钟一致。调用 ID 由宿主生成，在不同调用间不重用。每实例最多 4 个普通调用和 2 个专用审批调用；容量满时立即拒绝，不积累无界队列。

```json
{"type":"error","id":"unique-call-id","code":"scope_denied","message":"outside configured workspace","outcome":"rejected"}
{"type":"cancel","id":"unique-call-id"}
```

错误 outcome 可以为 `rejected`、`unknown`、`cancelled`。`rejected` 只能用于确认未开始该请求所代表持久操作的拒绝；一旦可能已创建会话、项目或派发操作而结果无法确认，必须返回 `unknown`。宿主对派发后的断连、协议破坏、未确认的取消、非正常退出及排空超时保留原调用 ID 和 Unknown，不自动重试。`cancelled` 也不证明已经产生的副作用撤销，因此首版映射为 Unknown。调用前已取消不会启动程序。

双向调用形态为：

```json
{"type":"callback","id":"callback-id","parent_id":"unique-call-id","method":"service.call","params":{"node_id":"observation-node","contract":"com.example.records","version":1,"method":"query","params":{"target_key":"target-001"}}}
{"type":"result","id":"callback-id","result":{"entries":[]}}
```

每顶层调用最多 64 次串行回调、130 条消息；回调 ID 不可重复，也不能冒用顶层 ID。处理回调时继续读取协议，拒绝未关联或并发回调。插件 `service.call` 只能路由至其 `allow_nodes` 所列节点，且所调方法必须注册、被配置允许并声明只读；不允许跨插件递归或通过通用接口调用 `recuvora.*`。

新业务首版仅支持只读插件方法。第三方程序可以实际执行消费/格式转换/校验代码并返回结果，无需重编宿主；宿主会验证输入与输出 schema。schema 不授予文件或网络权限，也不能防止已被部署者信任的恶意程序自行使用其操作系统身份。扩展写操作、通用远程执行许可、安装市场、持久热更新、断线续接及自动结果对账尚未实现。

## 支持的 schema 子集

schema 必须是对象并含一个字符串 `type`，支持 object、array、string、integer、number、boolean、null；不支持联合类型。可用关键字为 `properties`、`required`、布尔 `additionalProperties`、`items`、`enum`、`maxLength`、`maxItems`、`minimum`、`maximum`、`description`。其他关键字（包括 `$ref`、`oneOf` 等）明确拒绝，不宣称完整 JSON Schema 兼容。

schema 深度最多 12、单 schema 最多 64 KiB、对象属性最多 64、enum 最多 64 项。业务值最多 256 KiB、递归深度最多 24；字符串长度按 Unicode 字符计数；带 minimum/maximum 的数值及边界限制在 ±2^53 内，超出明确拒绝，避免浮点比较使大整数边界失真。schema 校验只证明载荷结构，不代替业务授权、真实目标验证或权威记录。

## Harness v1 映射

宿主 Harness 配置中的 `adapter` 为 `remote-node`，`address` 为 `node://扩展ID`，`workspace_roots` 对此适配器保存工作区 ID 列表（如 `work`、`review`），不保存宿主或远端绝对路径。代码使用 `RemoteWorkspace { node_id, workspace_id }`，由节点把 ID 映射到本机目录并执行本机范围与平台检查。宿主不得对远端路径调用本机 canonicalize。返回的原生项目 roots 仅是所属节点的路径元数据，不能用于宿主文件操作；消费层会话工作目录表示为 `node://ID/workspace`。

`run` params：

```json
{
  "harness_id":"selected-harness",
  "workspace":{"node_id":"harness-node","workspace_id":"work"},
  "prompt":"请求文本",
  "model":null,
  "visibility":"hidden",
  "placement":{"type":"none"},
  "role":"execution",
  "tools":[]
}
```

visibility 为 hidden/client；placement 为 `{type:"none"}` 或 `{type:"existing",project_id:"..."}`；role 为 execution/approval。审批必须新建 Hidden、无工具、无原生项目的独立会话。执行和审批不能复用上下文。工具定义为 `{name,description,input_schema}`，继承 Harness 的 32 个定义、合计 64 KiB schema 限制。

结果字段为 `thread_id`、`session_id`、节点本机绝对 `project_directory`、实际 `visibility`、`native_project_id`（可空）、`client_project_grouping` 和 `final_response`。grouping 为 `{type:"not_applicable"}`、`{type:"unverified"}` 或 `{type:"confirmed",client_project_id:null或字符串}`。原生项目归属不能冒充独立客户端观察。

`projects` params 为 `{workspace:{node_id,workspace_id}}`，结果为最多 512 个 `{id,name,roots:[节点绝对路径]}`。`create_project` params 增加 `name` 和 `idempotency_key`，返回单个相同项目对象。创建只注册节点上已有且允许的工作区，不创建目录；节点须验证请求与真实根匹配。

工具回调使用 method `tool`，params 为 `{harness_id,thread_id,turn_id,call_id,tool,arguments}`。结果为 `{content,success}`。宿主核验所选实例、线程、轮次、工具名和重复调用；参数最多 64 KiB，文本结果最多 256 KiB。只有该执行请求显式携带的可信 `HarnessToolHandler` 能接收回调。所有副作用仍由 handler 核验政策与一次执行许可，并记录意图及结果；节点不得通过模型原生 shell/文件/网络通道绕过这一路由。

CLI 使用 `harness ... --config ... --extensions ... --workspace ID` 选择远端工作区。宿主只通过 `remote-node` 调用 Harness，不把节点路径解释为本机 `--cwd`。普通第三方插件消费通过 `ExtensionRegistry::call_read_only`；其输出不是权威授权或业务恢复完成记录。

## 验证边界

集中集成测试使用测试程序自身子进程，覆盖第三方消费插件注册与节点读取、schema/权限/命名空间拒绝、协议身份和版本、跨平台路径归属、Harness 文本/项目/审批/工具以及断连和取消 Unknown。夹具不属于产品节点，不联系真实模型。跨机器 SSH 部署、macOS 节点平台后端、长期稳定性和真实业务恢复需单独验收。
