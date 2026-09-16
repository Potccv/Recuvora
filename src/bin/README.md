# 可执行程序入口

本目录保存单个 `recuvora` Cargo 包的附加可执行程序入口。所有入口保持轻量，只选择并调用 [`boot`](../boot/README.md) 中对应的装配函数；业务状态机、授权判断和平台能力仍归所属模块。

## 已登记目标

- `recuvora` 使用 [`../main.rs`](../main.rs)，由默认启用的 `cli` feature 提供现有 CLI。
- `recuvora-desktop` 使用 [`recuvora-desktop.rs`](recuvora-desktop.rs)，仅在启用 `desktop-ui` feature 时构建，并调用桌面启动函数。

`web-ui` 表示共享浏览器界面源码；`desktop-ui` 包含 `web-ui` 并增加 Tauri 2 桌面外壳；`all-ui` 同时选择两种前端交付形式。Web 与桌面端使用同一份 [`interfaces/ui`](../interfaces/ui/README.md) 页面资源，不维护第二套业务页面。

当前桌面入口只建立承载共享页面的窗口，不注册业务 command，也不改变现有 CLI 的授权或执行边界。Tauri bundling 暂未启用，因此尚不生成安装器。

开发约束见 [AGENTS.md](AGENTS.md)。
