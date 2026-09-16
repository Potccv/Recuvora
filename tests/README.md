# 测试

本目录集中维护宿主框架、恢复、监控、协议、接口和平台边界测试。供应商私有协议、进程监督与提供方集成验证由相应独立节点项目维护；宿主只测试通用 `remote-node` 契约和受限进程夹具。

集成测试统一由项目根单一 Cargo 包登记。测试临时文件必须写到 `RECUVORA_TEST_TEMP` 指定的源码目录外专用根；每个测试创建唯一子目录，只清理自己记录的路径。真实部署与活动目标不属于自动测试清理范围。

## 覆盖范围

| 文件 | 主要验证 |
| --- | --- |
| [framework.rs](framework.rs) | 类型化服务、依赖、作用域、生命周期、事件限额、调用监督与资源清理 |
| [recovery.rs](recovery.rs) | 模拟任务状态、授权、互斥、取消、超时、未知结果与持久恢复 |
| [incidents.rs](incidents.rs) | 监控检查点和故障的原子持久化、确认、解除、容量及损坏拒绝 |
| [monitors.rs](monitors.rs) | 只读轮询、规则、新鲜度、覆盖、动态发现、重启与取消排空 |
| [monitor_wire.rs](monitor_wire.rs) | 通过真实 JSONL 进程夹具消费监控批次、断连和非法响应 |
| [approval.rs](approval.rs) | 审批政策、委托范围、一次许可、Unknown、重放恢复和独立审批角色 |
| [harnesses.rs](harnesses.rs) | 多实例 registry、配置校验、项目归属、可见性、角色隔离、取消和结构化错误 |
| [cli.rs](cli.rs) | Harness CLI 参数、远端工作区、配置选择、JSON 输出、退出码和取消语义 |
| [extensions.rs](extensions.rs) | 外部扩展握手、契约/schema、方法白名单、生命周期、限额及恶意帧拒绝 |
| [remote_harness.rs](remote_harness.rs) | 通用 Harness 节点文本、项目、工具回调、审批隔离、超时与 Unknown 映射 |
| [process_recovery.rs](process_recovery.rs) | 测试宿主强制退出后的持久记录恢复与锁释放 |
| [server_logs.rs](server_logs.rs) | HTTP 认证、日志增量读取、目标隔离、诊断关联和插件监控视图 |

[interfaces.rs](interfaces.rs) 在 interfaces 模块的测试构建中引用；[ui.rs](ui.rs) 在启用 `web-ui` 时检查共享资产、安全渲染、审批状态目录和 Web/Desktop 装配不变量。它们不会扩大产品 API。[process_fixture.rs](process_fixture.rs)、[extension_fixture.rs](extension_fixture.rs) 和 `server_*.rs` 是父测试使用的辅助文件，不单独成为产品入口。

JavaScript 回归覆盖共享客户端、差异显示和观测记录行为：

- [ui_client.mjs](ui_client.mjs)：认证、连接、权限、Unknown、表单与页面状态协调。
- [ui_diff.mjs](ui_diff.mjs)：安全文本差异和大输入限额。
- [project_logs.mjs](project_logs.mjs)：增量观测记录、流切换、暂停和目标切换。

所有外部服务默认使用受限协议夹具，不需要真实凭据。夹具覆盖握手、乱序或迟到响应、输出上限、非法 schema、重复调用、断线、取消和未知持久结果；这证明宿主代理行为，不等于真实供应商服务、跨机 SSH、完整进程树或业务恢复验收。

## 运行

推荐从项目根使用统一脚本：

```powershell
./scripts/check.ps1 -BuildDir $env:CARGO_TARGET_DIR -TempRoot $env:RECUVORA_TEST_TEMP
```

脚本依次运行：

```text
cargo fmt --all -- --check
cargo clippy --all-targets --features server,web-ui --locked -- -D warnings
cargo test --all-targets --features server,web-ui --locked
```

共享客户端测试需要 Node.js，可用 `RECUVORA_NODE_PATH` 指定已有程序。宿主只有这一组默认回归；独立节点在各自项目中执行供应商协议测试。

手动运行 Cargo 时，同样必须先设置源码目录外的 `CARGO_TARGET_DIR` 和 `RECUVORA_TEST_TEMP`。只运行某个目标不能代表完整检查；涉及共享 UI 时还需执行 [UI 检查](../scripts/README.md#ui-检查)。

## 结果解释

- 格式、Clippy 和单元/集成测试通过，只证明当前源码在对应工具链和夹具下满足断言。
- 进程夹具证明协议与故障处理，不证明真实远程主机、模型服务或桌面会话可用。
- 文件写后读回只证明受限文本结果，不证明目标业务已经恢复。
- HTTP 和静态客户端测试不等于浏览器矩阵、Tauri 窗口或跨机 TLS 部署验收。
- 短时并发与反复启停不替代长期资源和无人值守稳定性验证。

当前宿主能力与自动验证边界集中在[实现状态](../docs/implementation-status.md)。供应商专属冒烟、版本、会话、活动实例和真实环境验收不属于宿主测试记录，由相应独立节点或集成项目保存证据。共同边界见[架构](../docs/architecture.md)与[插件设计](../docs/plugins.md)，开发约束见 [AGENTS](AGENTS.md)。
