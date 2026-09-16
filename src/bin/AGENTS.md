# 可执行程序入口开发规范

继承 [源码模块规范](../AGENTS.md) 与 [项目规范](../../AGENTS.md)，入口职责见 [README.md](README.md)。

- 所有产品可执行目标统一登记在项目根 `Cargo.toml`，不得在本目录创建 Cargo manifest 或子包。
- 入口只调用 `boot` 中对应的装配函数，不实现业务状态机、授权裁决、持久记录或平台操作。
- 每个可选入口必须声明准确的 `required-features`；新增文件不能依赖 Cargo 的自动 binary 发现。
- 桌面外壳复用 `interfaces/ui` 的 Web 页面，不复制路由、组件、状态或审批逻辑。
- Tauri command、插件和能力权限按最小范围逐项设计；没有已实现需求时不注册占位 command 或扩大前端权限。
- 界面加载成功只证明外壳可显示，不能写成完整恢复、远程管理或业务健康能力已经实现。
