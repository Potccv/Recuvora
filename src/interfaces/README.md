# 用户交互能力

本目录维护 Harness CLI 消费者、[repair CLI 参数](repair.rs)，以及 Web 与 Desktop 共用的 [UI](ui/README.md)。CLI 与官方 UI 均提供标准业务入口；HTTP 认证、装配和操作持久记录位于 [server](../server/README.md)。

`repair.rs` 同时负责将 `recovery::workflow::RepairResult` 转换为 CLI/HTTP 共用的 JSON 表达。业务模块不依赖界面；真实错误及关联标识在接口层转换，业务恢复未验证与禁止自动重放仍明确输出。

**Web 和 Tauri Desktop 共用一份 UI 与版本化 HTTP 客户端，已接入可信服务的快照、日志、Harness、模拟及审批修复入口。** 默认真实模式先显示独立认证页，成功验证服务地址与访问密码 / Bearer 令牌后才显示业务导航，并返回原先请求的页面；401 清除身份数据并回到认证页。演示数据只在显式演示模式使用。授权判断仍在可信服务，真实文件动作限 Windows 本机白名单文本替换，不支持任意命令或完整自愈。CLI 用法见 [审批说明](../../docs/approval.md)；HTTP 身份、revision、权限与失败语义见 [控制台 API](../../docs/console-api.md)。

## 共享 Web 与 Desktop UI

`ui/public/` 是唯一前端资产源，包含 HTML、CSS、ES modules 和图标。认证、概览、监控与故障、目标观测记录、修复任务、审批、Harness、宿主日志查询、模拟实验室、能力中心、连接设置与关于页面在浏览器和 Tauri 中保持相同；载体差异不分叉功能和内容。

官方 UI 基础壳是完整的标准控制台，Harness、修复、日志和模拟等页面随官方 UI 交付，不要求另行安装 UI 插件。日志查询支持来源、级别、任务、操作、文本筛选及有界分页；结果只来自已认证服务或明确演示模式。

快照默认每 5 秒持续串行读取，短暂离线标明旧数据并退避；区域更新保留控件、焦点和展开状态，切页返回恢复有界的内存草稿。目标观测记录页默认每 2 秒独立增量读取，普通记录与按配置分类的错误记录分别显示，离开页面取消读取；来源连续性变化或游标失效要求显式重读。相关操作回执随对应任务页面显示，全部操作记录放在宿主日志页的折叠区域。能力中心分别按实现阶段和运行可用性筛选。

后台能力可以来自内置业务模块、宿主侧插件或独立外部节点，其实现位置不决定标准页面必须插件化。官方页面通过统一的版本化服务契约获取能力清单、权威视图和操作入口，不复制审批与执行逻辑；缺少后端提供方时显示未配置或不可用，不以安装额外 UI 插件作为使用标准页面的条件。宿主已实现插件监控专页的受限声明式字段视图；任意 HTML/JavaScript、iframe、前端资源包或其他通用插件 UI 加载机制仍未实现。

UI 始终展示连接状态和最近数据时间，离线时保留陈旧快照并禁用提交。审批绑定服务返回的 request ID、revision 和 allowedActions；副作用 POST 不自动重试，断连保留 Unknown 与已知操作 ID。页面中的“已配置”不代表已探测或认证成功，文件读回一致不代表业务恢复。配置安装与修改仍由可信宿主管理，页面不能扩大权限。

Web 构建把共享资产复制到项目外的内容标识目录，Desktop 目标由同一 Cargo 包的 Tauri 薄壳嵌入相同资产。可用 `scripts/build-ui.ps1` 选择 `Web`、`Desktop` 或 `All`，具体命令、产物和工具要求见 [脚本说明](../../scripts/README.md)。项目不维护第二套前端、Node 包、前端开发服务器或第二个 Cargo manifest。

官方维护标准业务页面不意味着宿主依赖 UI 运行。Web 与 Desktop 保持可选交付，宿主可无界面运行；界面关闭不得成为后台任务或节点连接停止的原因。

两个载体使用同一 Bearer HTTP API，令牌仅保存在页面内存，不携带 Cookie、不跟随重定向或持久缓存业务响应。服务首版只绑定回环地址；跨机通过 TLS 反向代理或 SSH 转发，Desktop 可连接显式 HTTPS 或本机回环服务。没有 Tauri 业务 command、WebSocket 或多用户会话管理，不能将页面或窗口状态当作授权。

## 已实现的 Harness 命令

```text
recuvora harness list --config PATH --extensions PATH
recuvora harness projects --config PATH --extensions PATH [--harness ID] --workspace ID
recuvora harness create-project --config PATH --extensions PATH [--harness ID] --workspace ID --name NAME --idempotency-key KEY
recuvora harness run --config PATH --extensions PATH [--harness ID] --workspace ID --prompt TEXT --visibility client --no-project
recuvora harness run --config PATH --extensions PATH [--harness ID] --workspace ID --prompt-file PATH --visibility client --project ID
recuvora harness run --config PATH --extensions PATH [--harness ID] --workspace ID --prompt TEXT --visibility hidden --no-project
```

- 所有命令必须指定 `--config PATH`；远端调用还必须指定 `--extensions PATH` 和节点工作区。不能通过 Harness 配置安装或下载提供方代码。`list` 展示配置、默认项与装配状态，但不检查节点内的提供方认证或真实运行状态。
- 调用命令支持 `--timeout-secs N`，范围为 1–1800 秒，默认 180 秒。一次 `run` 的项目探测与实际调用共用此预算，不为第二阶段重置。
- `run` 必须明确 `--visibility client|hidden`，并且只选 `--project ID` 或 `--no-project` 之一。`hidden` 不能选择原生项目。省略 `--harness` 只使用配置默认项，找不到默认项或指定实例失败时不回退。
- 原生项目 ID 只在所选 Harness 内有效；`run --project` 先向该实例探测并确认 ID，再提交会话。探测失败或项目不存在时停止，不当成空项目列表继续。
- `create-project` 只要求节点在已选 `--workspace` 内执行提供方项目登记，不在宿主创建目录，也不隐含文本调用。幂等键是可核对的请求关联信息，CLI 不自动重试。
- 文本来源 `--prompt` 和 `--prompt-file` 二选一，必须为非空 UTF-8，最多 64 KiB；文件读取有上限。文本调用可指定 `--model MODEL`。路径在请求中保留操作系统编码，不通过有损转换再执行。
- 参数解析拒绝未知、重复、无关、缺失和空参数；配置与调用错误由启动入口按退出码报告。

成功输出使用 JSON，文本调用包含 `harness_id`、`session_id`、`thread_id`、节点资源形式的 `cwd`、`visibility`、`native_project_id`、`client_project_grouping` 和 `final_response`。提供方原生项目归属与独立客户端归组分别展示：客户端归组未经观察时保留 `unverified`，不能因原生项目响应而声称客户端归组已生效。隐藏会话的客户端归组为 `not_applicable`。

错误 JSON 包含稳定 `code`（`category` 同值）、可读 `message` 和 `auto_retry: false`。确定失败为 `status: failed`，提交前取消为 `canceled`；已经提交且无法确认结果时保持 `unknown`，展开已知 Harness、线程、工作目录、可见性、原生项目 ID 或项目名与幂等键。取消和超时不等于外部副作用已撤销。全部不可信文本经 JSON 字符串转义，不渲染为终端控制序列或执行指令。

## 职责与组织

- 本域组织交互服务契约、入口提供方及视图消费者，按实际入口组织子模块；通过依赖和配置选择实现。
- 界面展示权威业务服务提供的任务、证据、授权请求和能力状态，不维护另一套权威任务状态机。
- 静态 fixture 只用于显式演示模式，与真实权威数据严格分开；Web 与 Desktop 不各自维护 fixture 或业务规则。
- 人工入口可以替换，认证、授权最终裁决和持久记录仍由权威业务服务控制。
- 连接状态、数据时效与待配置能力应明确显示，避免将离线缓存呈现为当前事实。
- 内置入口不能独立加载任意新代码；独立扩展经元数据、实例握手及能力校验接入。事件订阅、连接及缓存绑定实例，卸载清理不撤销已接受的人工决定。

## 输入、输出与权限

- 输入是经授权的任务视图、证据引用和权威业务服务生成的待授权请求，内容包含目标、操作、范围、有效期及处理模式。
- 输出是绑定已核验用户身份、请求标识和版本的决定或管理请求；请求已发送与授权服务已接受决定分别展示。
- 仅获得所需的读取与请求提交能力；不能仅凭插件声称“用户同意”签发授权或直接驱动目标操作。
- 修复授权与 Recuvora 插件安装、升级和配置管理分别呈现，防止普通任务决定隐含扩大管理范围。

## 失败与验证计划

- 身份不可验证、请求过期或版本变化时保持待授权；界面失联不自动批准、拒绝或重写已记录的任务结果。
- 决定提交采用稳定请求关联和重复检查，连接恢复后以权威业务服务记录核对状态，防止重复授权或旧页面覆盖新决定。
- 实现后验证身份绑定、过期请求、重放、撤销、并发决定、断线重连及不可信证据内容的安全展示。
- [集中消费者测试](../../tests/interfaces.rs) 使用内存提供方覆盖严格参数、UTF-8 文件边界、默认及显式选择、项目探测顺序、不回退、共同截止时间、取消与未知结果展示；默认测试不会启动外部提供方。客户端可见性与归组仍需相应受支持集成验证。
- UI 行为测试使用模拟 HTTP 和最小 DOM 覆盖连接、权限、revision 冲突、Unknown、取消、有界查询及安全文本展示，不启动外部模型或浏览器。浏览器环境兼容、Tauri 窗口、外部节点和长期部署验收仍须单独报告。
- `server` 和 `web-ui` 可同时启用以托管共享资产；构建产物仍只写项目外，不提供未经部署的地址。

开发约束见 [AGENTS.md](AGENTS.md)；认证与权限边界见 [插件规范](../../docs/plugins.md) 和 [架构设计](../../docs/architecture.md)。
