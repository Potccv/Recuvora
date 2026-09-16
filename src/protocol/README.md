# 跨进程与跨节点协议

本模块已实现外部扩展 JSONL v1：版本化握手、身份绑定、有界帧、调用/错误、双向回调、取消和 Unknown 结果。共同契约与消息示例见 [外部扩展协议](../../docs/extension-protocol.md)。

传输使用可信配置指定程序的 stdio；可以运行本机独立扩展，也可以使用部署方配置的 SSH 命令连接远端。SSH 负责认证与加密，协议身份回显不能替代传输认证。宿主不提供节点程序，不实现裸 TCP 或 WebSocket 服务。

`ExtensionClient` 每次探测或顶层调用创建独立连接；调用 supervisor 持有容量、传输进程和在途回调。调用者丢弃 future 会请求取消，supervisor 继续等待可信 handler 和清理直接子进程。帧、消息、回调、参数、结果、握手、写入和排空均有边界；handler 本身须提供有限截止时间并在返回前记录持久结果。

通用协议直接使用 `framework::Cancellation`，不依赖 Harness 领域。节点注册表与共享宿主负责服务级派发关闭和调用排空，本层负责单连接的取消、回调与传输清理；这些职责不能替代远端执行事实核对。当前实现与验收范围统一见 [实现状态](../../docs/implementation-status.md)。

`validate_schema` 与 `validate_value` 实现文档列出的严格 schema 子集；未知关键字拒绝，不声明完整 JSON Schema。第三方契约归属与允许调用由 [nodes](../nodes/README.md) 核验。普通插件方法当前仅允许只读，写操作只能由已存在的可信 Harness 工具处理链执行。

请求可能派发后的连接丢失、取消未确认、协议错误和排空异常保留原调用 ID 与 Unknown，不自动重试。终止传输子进程不能证明远端执行者已停止；节点须自行监督本机后代并保留必要事实。自动重连、持久会话恢复和通用远程副作用对账尚未实现。

验证代码位于 [extensions](../../tests/extensions.rs)、[remote_harness](../../tests/remote_harness.rs)，使用测试程序自身子进程。协议夹具不代表真实跨机 SSH、模型或业务恢复验证。本域开发规则见 [AGENTS](AGENTS.md)。
