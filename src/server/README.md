# 统一控制台服务

新增[通用监控与故障](../../docs/monitoring.md)接口：可选 `monitors_config` 随共享宿主启动只读轮询，GET 查看监控及分页故障，POST 带 revision 确认收到。权限分别为 `monitor.read`、`incident.read`、`incident.acknowledge`。后台状态不依赖 UI；人工确认不解除故障或派发修复。HTTP 投影在 `monitoring.rs`，权威故障在 `recovery::incidents`。

监控摘要同时返回 `discoveries`：各发现源的清单完整性、已登记/当前发现目标数、最近接收与错误。发现失败不能作为空清单，自动登记不会增加调用白名单或修复权限。

监控摘要也列出每个配置的插件及其通用专页状态。固定 `GET /api/v1/monitoring/plugins/{id}` 需要 `monitor.read`，返回宿主拥有的插件状态、该业务契约所属监控和发现；只有同时具有 `extension.read` 才会调用插件已登记的 `describe_monitoring_view`。插件声明缺失、无权限、调用失败或校验失败时仍返回通用页面数据，并用独立 `view_status` 表达降级；该 GET 不自动重试插件调用。

插件视图响应由宿主按严格 schema、32 KiB、8 个 section 和 64 个 field 复核，只允许从返回的 `MonitorSnapshot` 或 `last_value` 取值的字段声明。HTTP 不提供插件 HTML、JavaScript、CSS、Markdown、URL、按钮或动作入口，所有声明字符串只按纯文本交给官方渲染器，浏览器也不直连插件。监控投影保留实际 provider 的 `extension_id`，并根据注册业务契约补充 `owner_plugin_id`；插件页面失败不改变监控或故障权威状态。

`monitoring.counts` 与故障列表的 `counts` 来自完整集合，故障筛选 target_id、monitor_id、kind、query 不改变全局计数。目标观测记录入口为 `GET /monitors/{id}/logs`，要求 monitor.read、logs.read 与 extension.read，由默认空的可信 `log_sources` 将已登记监控参数映射到外部只读契约。返回有界记录及按 `error_levels`/`error_events` 显式分类的错误记录；两项默认均为空，提供方问题单列 `source_error`。响应使用 `stream_label` 与 `available_streams` 表达记录流，不暴露底层存储文件。页面续读游标绑定目标与来源，最多256个且30分钟不用即失效，来源连续性变化明确返回409，不能接受前端任意资源路径或命令。

`server` feature 提供 `recuvora serve --config PATH`，`web-ui` 同时启用时托管官方共享 UI。无窗口运行不依赖 Tauri。

启动配置和运行数据必须在源码外，配置显式指定身份、令牌文件与权限。HTTP 服务只绑定回环地址，局域网部署使用 TLS 反向代理或 SSH 端口转发。令牌只由 UI 保存在内存。

服务提供 Harness 项目查询及会话调用、已配置的修复流程、带 revision 的人工审批与执行、封闭模拟实验、操作查询与取消，以及真实运行记录的有界日志查询。插件消费通过宿主登记的契约路由。未知结果不自动重试；操作列表记录传输结果，审批和模拟各自的持久服务保留业务权威。

`bootstrap` 只返回每个历史集合的首批摘要；操作、修复、审批和模拟使用 ID 游标分页，全文由单条详情读取。待审批队列独立于已结束历史，计数来自完整持久记录。审批摘要不包含 `allowedActions`，授权操作必须依据完整详情中的准确 revision。摘要投影在存储锁内读取，避免为列表复制全部审批原文。

手动修复请求可以用成对 incident_id、incident_revision 绑定故障来源，宿主检查故障读取权限、持久 revision 及固定目标/政策范围后才接受；关联保存在现有操作上下文，终态或重启不能改写。修复详情返回 sourceIncident，故障详情返回最多25条 related_repairs 与完整关联数量。旧记录继续兼容；关联不产生审批、恢复事实或自动派发，同一控制台 task_id 不重复运行。

内部边界：`mod.rs` 管控制台状态和共享宿主生命周期，`http.rs` 管路由、认证、来源与静态交付，`handlers.rs` 验证 HTTP 输入并派发可信服务，`history.rs` 管有界摘要、游标及按需详情，`monitoring.rs` 投影监控/故障，`project_logs.rs` 负责可信目标观测记录映射与有界续读，`views.rs` 生成能力和审批展示视图，`journal.rs` 管持久调用回执。Harness 与扩展装配复用 `boot::host`，不再借用 CLI 实现。静态 HTTP 交付使用 UI 模块维护的共享资产目录。

配置与接口示例见[控制台 API](../../docs/console-api.md)。宿主调用日志与目标提供方观测记录使用不同入口，后者需要配置已注册的只读扩展与可信记录映射。自动测试使用合成记录与隔离协议夹具，不能代替任何目标专属集成或远程部署验证。
