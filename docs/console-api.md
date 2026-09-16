# 官方控制台与 HTTP API v1

官方 Web 和 Tauri Desktop 共用一份页面及 HTTP 客户端。监控与故障、Harness 会话、修复详情、审批、日志查询、模拟实验室均由官方 UI 提供。每个配置插件可进入宿主管理的监控专页；插件只能登记受限声明式内容，不交付可执行页面。默认进入独立认证页，通过宿主验证后才显示业务导航与运行记录；演示模式必须从认证入口或设置显式选择。后端没有某项能力时显示未配置，不回退为演示成功。

## 构建与启动

```sh
cargo build --locked --features server,web-ui --target-dir <external-build-directory>
recuvora serve --config <external-console-config.json>
```

`server` 控制 HTTP 服务编译，`web-ui` 同时启用时嵌入共享静态资产。仅 `server` 可以为外部托管的 Web 或 Desktop 提供 API；默认 CLI 不引入 HTTP 和 Tauri 依赖。Harness 统一选择 `remote-node`，宿主不编译供应商专有实现，也不提供节点构建角色。

启动配置包含以下字段。路径必须指向已存在的项目外位置，示例路径须替换成部署机器的绝对路径；实际令牌不能提交到版本控制：

```json
{
  "schema_version": 1,
  "listen": "127.0.0.1:7431",
  "token_file": "/srv/recuvora/operator.token",
  "operator": "console-operator",
  "permissions": ["harness.run", "harness.projects", "repair.run", "approval.decide", "approval.apply", "approval.reconcile", "simulation.run", "logs.read", "operation.cancel", "extension.read"],
  "allowed_origins": ["http://tauri.localhost", "tauri://localhost"],
  "data_dir": "/srv/recuvora/console-state",
  "harness_config": "/srv/recuvora/harnesses.json",
  "extensions_config": "/srv/recuvora/extensions.json",
  "repair_config": null,
  "monitors_config": null,
  "log_sources": []
}
```

独立部署时按实际需要缩减 permissions；未知权限拒绝启动。`harness_config`、`extensions_config`、`repair_config` 和 `monitors_config` 可以为 null。修复未配置时仍可使用模拟与日志；监控需配置对应只读提供方。修复动作当前只实现 Windows 白名单文本替换，Linux 宿主不能因此声称拥有 Linux 文件修复能力。

令牌为独立随机产生的 32 至 256 字节可打印 ASCII 秘密，令牌文件权限由操作系统限制。请求使用 `Authorization: Bearer <token>`；服务端配置的 operator 是可信审计身份，客户端不能在决定正文中指定操作者。当前为单操作员令牌模式，尚未提供多用户登录或 OAuth。

认证页的“访问密码 / 令牌”即上述 Bearer 秘密，不是新增账户密码。认证使用 `GET /bootstrap` 验证，验证中或失败时不显示业务壳；成功后返回最初请求的页面。认证失效的 401 会取消读取并清除令牌、详情、回执与页面草稿，回到认证入口。令牌不写入浏览器持久存储。

服务只接受回环监听；跨机浏览器或 Desktop 通过 HTTPS 反向代理或 SSH 端口转发访问。反向代理须自行落实 TLS、连接和速率限制。同源 Web 不需额外 CORS；Desktop 或独立托管站点填入准确 allowed_origins，不允许通配符。来源检查不代替令牌认证。UI 令牌只在当前页面内存保存；断开连接清除令牌。

## 接口

所有业务入口使用 `/api/v1` 前缀。JSON 请求体上限 128 KiB；错误含 `error.code`、`error.message`、`auto_retry:false`。请求已到达服务端后，即使浏览器超时也可能继续执行。

| 方法与路径 | 功能与输入 |
| --- | --- |
| `GET /bootstrap` | 官方 UI 初始化快照：runtime、permissions、capabilities、harnesses、active_configuration，以及四种历史集合的首批摘要、pages 元数据和 approval_counts；schema_version=1 |
| `GET /repairs`、`GET /approvals`、`GET /operations`、`GET /simulations` | 摘要分页；cursor、limit、state、query、task_id；返回 items、next_cursor、total、limit、order |
| `GET /repairs/{id}`、`GET /approvals/{id}` | 单条完整详情；修复回复、审批全文和政策按需读取 |
| `GET /harness/{id}/projects?workspace_id=...` | 所选实例的项目探测；返回 projects，探测失败保留错误 |
| `POST /harness/{id}/runs` | operation_id、workspace_id、prompt、visibility（hidden/client）、project_id、model、timeout_secs |
| `POST /repairs/runs` | operation_id、task_id、prompt；可选成对 incident_id、incident_revision；使用服务端既定目标、政策及执行/审批实例 |
| `POST /approvals/{id}/{approve,deny,revoke,apply,reconcile}` | revision、reason；批准和执行为不同动作，各自检查权限及权威版本 |
| `POST /simulations` | operation_id、task_id、target、scenario、timeout_ms；仅封闭模拟 |
| `GET /operations/{id}` | 查询已接受请求的状态与结果 |
| `POST /operations/{id}/cancel` | 请求取消，最终状态需继续查询；不会将已发送操作立即报告为确定取消 |
| `GET /logs` | level、source、task_id、operation_id、query、cursor、limit；返回 items、next_cursor |
| `POST /extensions/{id}/query` | contract、version、method、params；只调用配置及注册契约共同允许的只读方法 |
| `GET /monitors`、`GET /monitors/{id}` | 监控摘要/详情；要求 monitor.read |
| `GET /monitoring/plugins/{id}` | 宿主管理的插件监控专页数据；monitor.read 返回通用监控，extension.read 才允许读取已登记的声明式视图 |
| `GET /monitors/{id}/logs` | 目标提供方的只读观测记录流；cursor、limit（1–32）；要求 monitor.read、logs.read 与 extension.read |
| `GET /incidents`、`GET /incidents/{id}` | 持久故障摘要分页/完整证据；要求 incident.read |
| `POST /incidents/{id}/acknowledge` | revision、note；要求 incident.read 与 incident.acknowledge，身份由服务端绑定 |

异步 POST 在持久化 operation_id 后返回 `202 {operation_id,auto_retry:false}`。调用方先生成唯一 operation_id，响应丢失后用 GET 重查。重复 ID 返回 409，不再次派发；ID 未找到也不是“远端没有动作”的通用证据。请求不可自动重试。

操作状态为 running、completed、failed、canceled、unknown。completed 表示该服务调用已结束，业务结果仍须查看 result；例如审批等待人工、模拟失败或文本内容读回不能被包装成业务恢复成功。时间字段为 Unix 毫秒。原始模拟结果保留既有契约的状态枚举，UI 快照转换为小写状态。

### 摘要、分页与详情

初始化每个历史集合最多返回 25 条摘要。`pages` 使用 `repairs`、`approvals`、`operations`、`simulation_tasks` 四个键，分别含 `next_cursor`、`total`、`limit`、`order`，不重复包含记录。操作摘要没有 `result`；审批摘要没有文件全文、完整政策、评审理由或 `allowedActions`；修复摘要没有模型最终回复。摘要展示文本最多保留 256 个字符，准确原文以单条详情为准。审批原文必须完整读取后才能提交带 revision 的决定，摘要不能用于批准。

`bootstrap.approvals` 独立查询 `state=attention`，覆盖 pending、waiting_human、approved 和 unknown，已结束历史不会挤掉待处理记录。`approval_counts` 是各状态的完整数量，不使用当前页长度推算全局指标。完整审批历史通过 `GET /approvals?state=all` 读取；修复详情中的相关审批通过 `task_id` 筛选并单独分页。

列表默认每页 25 条，`limit` 接受 1 至 100。`state` 支持准确状态名、all 或 attention；`query` 搜索摘要文本，最长 256 字节。记录按不可变 ID 字典序降序排列，`cursor` 使用上一页返回的 `next_cursor`，后一页只包含比该 ID 小的记录。操作结束时的更新时间变化不会把记录移到已经读过的页。分页读取当前持久状态，不是冻结快照：新增 ID、状态及匹配数量可能变化；刷新首页取得新记录。模拟列表的 `id` 为调用 ID，`taskId` 为模拟任务 ID。

UI 每个集合只保存当前摘要页，最多缓存 8 条修复/审批详情和 16 条本次操作回执；筛选与翻页不会因后台初始化轮询被重置。全局快照默认每 5 秒持续串行读取，每次响应、超时和缓存容量保持有界，没有固定 120 轮停止限制；短暂失败按最多 30 秒间隔退避，页面隐藏或切换身份时取消读取。列表和日志结果显示各自读取时间，最新全局快照时间不覆盖较早查询的时间。

同一页面通过区域协调更新状态、计数和行，保留已有导航、编辑控件、焦点和证据展开状态；列表与目标观测记录分别更新其持久区域。页面内存最多保存 12 个页面的非密码表单草稿，切去记录页再返回可继续编辑，关闭页面、断开或切换身份时清除。审批详情失效先禁用旧动作，完整 revision 读取后重新显示允许动作；409 后同样重新读取详情，不只刷新列表。新 revision 替换差异证据，保留审批理由供重新审查。上述读取均为 GET，不重复提交副作用。

具备可信观测记录源和读取权限的目标可进入独立记录页。页面默认每 2 秒串行增量读取，积压记录使用短间隔续读，普通记录和按配置分类的错误记录分别显示在有界滚动窗口；离开或隐藏页面取消读取。来源连续性变化或游标失效会停止续读并提示明确重新读取，不能静默跳过记录或自动重新提交业务动作。

审批服务在同一存储锁内核对 revision 后决定、撤销或消费许可；旧页面得到 409 后必须刷新，不能覆盖新决定。Unknown 仅在可信核验目标前后内容后消歧，不重复写入。模型和第三方只读插件无法访问人工决定入口或构造执行许可。

### 手动关联故障诊断

修复运行请求可选择成对发送 `incident_id`、`incident_revision`。宿主要求 `repair.run` 与 `incident.read`，从持久故障服务读取准确记录，核对 revision，以及故障 target_id 与固定修复配置和政策允许目标完全一致后才接受。缺少其中一项或无效 revision 返回400，旧 revision 或目标越界返回409，不存在的故障返回404，权限不足返回403；拒绝时不接受操作、不调用 Harness。请求不能提交另一个目标或文件路径。

接受前将有界故障来源快照和固定目标写入现有 `operations.jsonl.context`。上下文在终态和重启中保持不变，旧无关联日志仍兼容，不变更审批存储格式。修复摘要/详情的 `sourceIncident` 返回该快照（id、revision、monitor_id、target_id、rule_id、kind、summary、状态/时间、captured_at），不带 arbitrary evidence 或人工备注；无关联为 null。后续故障新观测更新 revision 不会覆盖已经接受的来源快照，历史 target 来自该次接受记录。

故障详情与确认响应中的 `related_repairs:{items,total,limit:25}` 返回最多25条关联修复摘要，按不可变修复 ID 降序，total 为全部关联数量。同一 repair task_id 在控制台接受锁内拒绝再次运行；已有 CLI/审批持久记录的 task_id 即使没有控制台操作记录，也在接受前返回409，不能将新故障来源关联到旧任务。响应丢失时查询既有 operation_id 或修复详情，不生成新调用重放同任务。关联用于调查来源追踪，不确认收到、不解除故障、不产生审批或扩大授权，也不从监控轮询自动派发修复。

## 数据、限额与恢复

### 通用监控与故障

`bootstrap.monitoring.discoveries` 与 `GET /monitors` 的 `discoveries` 返回发现源状态：`id`、实际 provider `extension_id`、业务契约归属 `owner_plugin_id`、契约/版本/方法、`running`、`last_received_at_ms`、`complete`、`known_targets`、`present_targets`、`last_error`。`known_targets` 包含已登记但当前缺失的目标，不能作为当前运行数量；失败或部分清单不撤销已有目标。调用权限仍为 `monitor.read`，响应不包含宿主模板参数或授权凭据。

`monitors_config` 为可选的项目外配置路径；监控引擎随共同宿主启动，在 `data_dir/monitoring` 中持久保存故障及检查点。后台轮询不依赖 UI 页面。无此配置时返回未配置，已有配置但缺少提供方时报告覆盖问题。配置、批次契约、时效规则和持久边界见[监控说明](monitoring.md)。

`bootstrap.monitoring` 在具有 `monitor.read` 时返回 `configured`、`monitors` 摘要、`discoveries`、`plugins` 目录与 `runtime_error`，否则为 null。`GET /monitors` 将摘要放在 `items`；大值不放入初始化快照。监控摘要保留实际 `extension_id` 并增加按版本化业务契约解析的 `owner_plugin_id`。`counts` 返回完整监控数量 `monitors`、去重目标数量 `targets`、`unhealthy`、`stale`、`collection_issues`，以及有 `incident.read` 时的完整 `incidents:{open,acknowledged,resolved,active,total}`；无故障读取权限时 incidents 为 null。采集问题数量包含覆盖不完整或没有新鲜样本的监控，不等于目标故障数量。

`plugins` 每项只给出插件注册、专属视图登记、错误和监控/目标/发现数量，不在 bootstrap 中调用插件。`GET /monitoring/plugins/{id}` 一次返回该 owner 的完整有界监控快照、发现状态，以及可选 `view`；`view_status` 区分 ready、not_registered、invalid_registration、permission_denied、unavailable 与 invalid_response。视图方法固定接收 `{"schema_version":1}`，调用时禁用插件节点回调；宿主与浏览器再分别执行 32 KiB、8 个 section、64 个 field 及封闭字段/格式校验。声明只按 `view_role` 和 JSON pointer 读取宿主快照或 `last_value`，不支持 HTML、脚本、样式、Markdown、URL、按钮或动作。任何专属视图失败都保留通用插件页，且不改变健康、覆盖、故障、授权或恢复事实。

故障按需请求，默认每页25条，最大100，`status=active|all|open|acknowledged|resolved`，支持准确的 `target_id`、`monitor_id`、`kind=target|coverage|all` 与最长256字节的 `query` 摘要搜索；按不可变ID降序使用 `cursor` 分页。响应 `total` 是当前筛选匹配总数，`counts:{open,acknowledged,resolved,active,total}` 始终来自全部持久记录，与分页和筛选无关；`read_at` 为本次读取的宿主时间。摘要不带完整 evidence 或人工备注。

人工确认接口同步保存后返回200及完整故障，含 `allowed_actions`、`auto_retry:false`、`business_verified:false`。旧revision返回409；响应不确定时重新GET核查，不自动重发POST。确认只将open变为acknowledged，不解除故障、不调用修复模型、不创建控制台执行操作。新的有效观测满足恢复规则后才记录resolved，其含义为异常条件解除。

### 目标提供方观测记录

宿主调用记录继续使用 `GET /logs`。`GET /monitors/{id}/logs` 单独通过部署授权的外部只读契约读取目标提供方观测记录，宿主不接受前端提交的资源路径，也不接受客户端选择契约、方法、目标实例或 shell。监控摘要和详情返回 `logs_available` 与 `log_source_id`；所需权限、可信映射或已注册提供方缺失时记录流不可用。

可选 `log_sources` 默认为空。以下配置将独立只读提供方的 `query` 绑定到已登记监控的可信目标键，适用于静态监控和可信模板自动发现。扩展配置还必须允许该 `query` 方法；读取观测记录不赋予修复权限。

```json
{
  "log_sources": [{
    "id": "observation-records",
    "extension_id": "observation-extension",
    "monitor_contract": "com.example.observation",
    "contract": "com.example.records",
    "version": 1,
    "method": "query",
    "params": {},
    "parameter_bindings": {"target_key": "/target_key"},
    "cursor_parameter": "cursor",
    "limit_parameter": "limit",
    "timeout_ms": 5000,
    "error_levels": [],
    "error_events": []
  }]
}
```

每个 extension_id 与 monitor_contract 只能有一项记录映射，最多16项。固定 params 最多16 KiB，parameter_bindings 从可信监控定义派生有界标量身份，不能覆盖游标或条数参数。具体目标及提供方可读范围仍由对应只读契约限制。结果字段通过可选 `result` JSON pointer 映射；默认响应字段为 `/entries`、`/next_cursor`、`/has_more`、`/stream_label`、`/coverage`、`/error`、`/available_streams`，记录字段为 `/id`、`/timestamp`、`/level`、`/message`、`/event`。其他只读记录契约可配置自己的指针，不需要核心内置提供方解析器。

响应结构为 `items` 与 `errors`（均包含 id、timestamp、level、message、event）、`next_cursor`、`has_more`、`read_at`、`stream_label`、`coverage`、`source_error`、`available_streams`、`reset:false`，以及 monitor_id、target_id、source_id、log_source_id、extension_id。`errors` 只包含 `error_levels` 或 `error_events` 显式匹配的本次记录；两项默认均为空，宿主不硬编码任何等级。提供方调用失败、记录不可用与初始历史省略独立显示为 `source_error`，不能当作目标错误记录。页面内容遵循提供方的脱敏范围；宿主不假定提供方返回底层存储的完整原始内容。

首次省略 cursor，由提供方读取有界的最近记录；后续只传响应的宿主游标以增量续读。每次1–32条，提供方响应最大256 KiB，消息最大16 KiB，提供方游标最大4 KiB。游标为宿主内存中的来源/监控绑定句柄，最多保留256个，30分钟未使用或宿主重启后失效；不能直接传插件游标、改目标后复用旧句柄。来源连续性变化或游标失效返回409 `cursor_invalid`，需明确重新开始读取；不会静默跳过记录或伪造完整覆盖。提供方可通过 `available_streams` 报告可选择的记录流；流选择和跨流覆盖语义由已审查的只读契约定义。提供方返回 `has_more` 却未推进续读游标时拒绝继续派发热点轮询。

未配置可信记录源，或提供方路由未注册/未启用返回503 `logs_unavailable`，页面等待明确重新读取。已注册来源的暂时断线、超时、调用容量或游标暂不推进返回503 `logs_read_failed`，GET 可有限退避恢复；不会重放副作用。不存在的监控返回404。页面进入记录视图时读取并进行无重叠增量 GET 轮询，可手动暂停；离开、权限失效或断开认证后释放轮询，不改变后台监控任务。

控制台操作日志最多 1000 个操作、8000 条事件、64 MiB；单记录最大 1 MiB，并发操作最多 8 个。达到容量后拒绝新接受，停机后由运维归档。审批和模拟仍使用自己的权威存储和单写入者锁；控制台日志是调用记录，不替代业务许可。

HTTP 与 CLI 共用 `boot::host` 的真实服务装配和关闭流程。控制台停机先拒绝新派发，再取消并排空已接受调用，最后释放服务；未排空或清理失败以错误返回。初始化失败也通过共同生命周期回收已经启动的服务。

重启将未完成操作标记 unknown，不自动重放。`operations.jsonl` 若有不完整末尾或非法状态转换会拒绝启动并保留证据，不自动截断或修复，需要运维核对。宿主日志查询读取持久调用记录；目标提供方观测记录通过配置的外部只读记录契约查询，后台监控游标与页面记录游标相互独立。没有无限记录缓冲、插件安装市场或任意新领域写动作接口。

节点进程、SSH 与新业务契约见[扩展协议](extension-protocol.md)。当前自动验证边界记录于[实现状态](implementation-status.md)；协议测试不能替代目标专属远程集成和平台后端验证。
