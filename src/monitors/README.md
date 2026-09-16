# 通用观测与监控

本模块已实现配置驱动的只读轮询、确定性规则、样本新鲜度和采集覆盖检查。具体日志、进程、指标和健康探针由独立节点项目实现；通用接入复用 [nodes](../nodes/README.md)。故障记录和人工确认属于 [recovery](../recovery/README.md)，此模块不自动诊断或执行修复。

`MonitorHandle::definition` 提供已登记监控的可信只读定义，包含静态与动态模板派生的固定参数，供宿主日志等服务绑定目标；HTTP 摘要不暴露该配置。监控快照另保留实际 provider 的 `extension_id`、契约/版本/方法、可选 `view_role` 及详情中的完整最近观测 `last_value`，供服务器构造插件监控专页；该值仍受每条观测 4096 字节上限约束，不是绕过采集边界的第二份数据。定义登记与监控视图生命周期一致，独立的页面读取或日志续读不会推进监控引擎持久游标。

## 配置

`MonitorsConfig` 使用 `schema_version: 1`，配置最多 256 KiB。`monitors` 保存静态定义，可选 `discoveries` 默认为空。静态数量加自动发现预留目标数量最多 64。每项静态定义包含 `id`、`target_id`、`source_id`、可选 `view_role`、`extension_id`、`contract`、`version`、`method`、对象 `params`、`interval_ms`、`timeout_ms`、`stale_after_ms`、`startup_grace_ms`、`rule`。实例 ID 唯一，不能通过普通监控调用保留契约或写方法。`view_role` 只是插件专页按目标选择展示快照的稳定标识，不影响观察请求、规则、时效、覆盖、游标、检查点、故障或授权。

规则示例：`{"pointer":"/ready","operator":"eq","value":true,"failure_samples":2,"success_samples":2}`。JSON pointer 选择每条样本 value 中的字段；比较成立表示健康。bool/string 支持 eq/ne，number 另支持 gt/ge/lt/le；数字限 ±2^53，连续样本阈值 1–1000。字段缺失、错误类型或超限表示证据不可解释。每条故障信号保存完整规则，便于配置变更后的追溯；重启重新计数。

周期范围 10 ms–1 小时，调用期限 1 ms–30 秒，新鲜度与启动宽限 10 ms–24 小时。每监控只允许一个在途调用，排空后等待一个周期，不补跑积压队列；扩展注册表的共享容量限制继续有效。

## 通用发现与动态登记

`discovery.rs` 维护最多 8 个配置授权的发现来源。每项 `MonitorDiscovery` 包含 `id`、`extension_id`、`contract`、`version`、`method`、对象 `params`、`interval_ms`、`timeout_ms`、`max_targets`、`parameter` 和完整 `template: MonitorDefinition`。发现方法必须通过现有只读契约路由，不改变扩展注册表，不加载新的业务代码或扩大提供方的读取授权。

发现返回严格的版本化清单：

```json
{
  "schema_version": 1,
  "complete": true,
  "targets": [{"key": "target-001"}],
  "error": null
}
```

每批最多 64 个唯一 key，字符限 ASCII 字母、数字、下划线和连字符，长度 1–64。生成的监控 ID、目标 ID、来源 ID 分别为模板对应前缀加 `.` 加 key；观察参数在模板 `params` 中插入 `parameter: key`。模板不得预先含该参数，参数不能覆盖 `target_id`、`source_id`、`cursor` 或 `generation`。提供方不能通过发现响应选择程序、路由、规则或周期。配置预先检查 ID 长度、静态与动态 ID 冲突及预留容量；`discovery.` 监控 ID 前缀用于持久发现检查点，禁止静态或模板占用。

有效部分清单可以加入目标，但只有 `complete: true` 且 `error: null` 的完整清单能将缺失目标标为不在场。错误、重复 key、非法或超量响应整体拒绝，不新增部分目标，也不把失败当空清单。既有已确认在场目标可继续采样，发现来源自身的失败在 `MonitoringSnapshot.discoveries` 中显示。每条发现状态包含运行状态、最后成功接收时间、完整性、已知与在场数量及错误。

已发现 key 按来源终生累积在同一 `IncidentStore` 的 `discovery.ID` 检查点，登记新 key 先同步持久化，再发布监控和启动采样。`max_targets` 为该来源累计身份上限，删除目标不会释放这一容量；超过上限明确报错，不静默丢弃旧身份或新增清单的一部分。发现绑定包含来源路由、参数映射及观察模板的来源身份；绑定改变必须使用新的发现 ID，不能复用旧清单。监控自身继续核验完整来源绑定和既有游标。

删除目标保留监控、游标及未解除故障，独立检查周期最多 250 ms 后报告覆盖不可用和健康未知；不会用缺失清单产生解除事实。重现同一 key 复用其 worker 和检查点，仍需新鲜观测满足恢复条件。改名按新 key 登记，旧目标继续保持未知。重启从持久清单恢复监控身份与游标，但所有动态目标先置为未知且禁止真实观察派发，直至发现重新确认在场。尚未确认时遵守 `startup_grace_ms`，宽限内不会仅因重启创建覆盖故障，首次确认在场也不制造瞬时故障；超过宽限仍无新鲜样本时按既有时效规则报告。完整清单明确确认缺失后不再属于等待状态，应及时报告覆盖异常。

发现与观察共享关闭门及有界工作任务计数。空清单不会提前关闭存储；停止先拒绝登记并取消所有来源，再等待发现和观察排空。成员状态带递增代次，观察期间删除后重现也不能接受旧结果；结果提交与成员变化串行校验，迟到响应不能推进已失效观察的游标或把目标变健康。

`ObservationSource::discover` 默认报告不支持发现，测试或可信内置来源可覆写并返回 `DiscoveryBatch`；真实来源仍使用 `ExtensionRegistry::call_read_only`。`observation_epoch` 用于动态来源的在场性校验，`observation_pending` 区分尚未重新确认和明确缺失；普通来源默认恒定可用且不等待发现。等待阶段只运行时效检查，不创建观察请求，其他阶段仍保持每监控单个在途调用。发现不创建修复任务、写许可或业务成功事实。

## 观测契约

宿主向 params 加入保留字段 `target_id`、`source_id`、`cursor`、`generation`，配置不能覆盖。首个游标和代次为 null，后续来自已提交 checkpoint。外部只读方法返回：

```json
{
  "schema_version": 1,
  "target_id": "target-001",
  "source_id": "source-001",
  "generation": "source-generation-1",
  "cursor": null,
  "next_cursor": "position-1",
  "coverage": "complete",
  "has_more": false,
  "error": null,
  "samples": [
    { "id": "sample-1", "sequence": 1, "age_ms": 0,
      "value": { "ready": true }, "evidence": { "kind": "provider-observation" } }
  ]
}
```

cursor 回显请求位置，next_cursor 是节点解释的不透明非空位置，最多 4096 字节，不隐含删除或确认消费。generation 绑定数据源连续性，不因每次协议进程启动改变；来源连续性变化或状态丢失须明确报告新代次和缺口。宿主保存代次、序号、游标及有界去重标识，代次改变首批强制部分覆盖，不能直接恢复健康。

每批最多 32 条样本和 256 KiB；每条 value 与对象 evidence 各最多 4096 字节，整批两者合计最多 128 KiB，规则序列化最多 4096 字节，保证有序信号及 checkpoint 能原子写入日志。代次内序号递增；已处理的重复/旧序号不计数、不刷新年龄，批内倒序或同序号内容冲突拒绝整批。coverage 为 complete/partial；has_more、非空 error 或代次变化使覆盖为 partial。空完整批次只说明采集覆盖，不产生健康结论，不解除已有故障；空批、过期和非法样本打断连续计数。

age_ms 是节点组装响应时的样本相对年龄。宿主加上调用耗时，再用本机单调时钟累计，不直接比较跨机墙钟。历史样本只推进合法读取位置，不能刷新当前样本。节点须真实报告探测范围、年龄与丢失区间；schema 与连接成功不能证明业务事实真实。

## 状态、持久化与关闭

- 目标判断：unknown/healthy/unhealthy。
- 新鲜度：missing/fresh/stale。
- 覆盖：unknown/complete/partial/unavailable。

独立定时器在调用期间检查缺失与过期；期限到达先报告不可用并取消，再等待原调用收尾。故障分 Target 与 Coverage：新鲜违规样本达到阈值产生 Target，缺失、过期、断线与不可解释观测产生 Coverage。新鲜完整观测解除覆盖故障，达到恢复阈值才解除目标故障；人工确认不是恢复证明或授权。

批内信号按序与批末 checkpoint 原子提交到 IncidentStore，保留已达到阈值的发生/恢复转换；后续不充分或非法证据会撤销本批不能成立的末次解除。重复批且游标不变不追加相同记录。记录失败或容量耗尽停止采集并暴露 runtime_error。

`MonitorEngine::start` 使用真实 `call_read_only`；`start_with_source` 接收可信 ObservationSource，便于测试和已有宿主提供方。handle 提供监控与发现状态、故障查询及 revision 确认。begin_shutdown 停止新派发并取消，drain 等待监督收尾；shutdown 组合两者并释放文件锁。Drop 请求取消，后台监督保留在途所有权。来源必须合作取消、有界收尾，不能承诺任意外部代码都可安全强停。

重启保留 checkpoint 和未解除故障，目标从 Unknown 开始，重新获取样本并计数。来源身份变化要求新 monitor ID；规则变更保留规则证据，但不会继承旧健康结论。

## 验证范围

[monitors 测试](../../tests/monitors.rs) 使用可控来源与暂停时钟覆盖阈值、重复/倒序、空批、部分覆盖、过期、身份/类型错误、重启游标、代次改变、取消及确认。[monitor_wire 测试](../../tests/monitor_wire.rs) 实际经过引擎、测试插件、测试节点只读路由，验证持久故障、恢复证据和断连覆盖。

这些测试不提供目标专属采集器，也不代替远程集成、来源连续性或业务状态验证；当前自动检查边界见 [实现状态](../../docs/implementation-status.md)。开发约束见 [AGENTS](AGENTS.md)。
