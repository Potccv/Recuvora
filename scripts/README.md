# 构建与开发检查脚本

`check.ps1` 检查宿主默认组合的格式、Clippy（警告作为失败）和全部可用目标测试。供应商节点在各自独立项目中检查，不在宿主脚本中重复编译。需要 Rust 工具链及对应平台链接器；脚本不安装或修改系统工具。

```powershell
./scripts/check.ps1 -BuildDir $env:CARGO_TARGET_DIR -TempRoot $env:RECUVORA_TEST_TEMP
```

两个路径必须是项目外的目录。`TempRoot` 是专用测试临时根，各测试创建唯一子目录并自行清理；脚本不会递归清空整个临时根。可用 `-CargoPath` 指定 Cargo 可执行文件。

检查命令为 `cargo fmt --all -- --check`、`cargo clippy --all-targets --features server,web-ui --locked -- -D warnings` 和 `cargo test --all-targets --features server,web-ui --locked`，包含真实 HTTP 服务、共享 UI 客户端、interfaces、workflow、extensions 和 remote_harness 等宿主测试，需要 Node.js。测试目标统一登记在根 `Cargo.toml`，无需选择工作区成员。

## 共享 UI 构建

[build-ui.ps1](build-ui.ps1) 从 [唯一静态 UI 源](../src/interfaces/ui/README.md)构建可选前端。`Web`、`Desktop` 和 `All` 分别生成 Web 静态目录、Tauri Desktop 可执行文件，或两者；三个目标的页面、路由、样式和演示数据都来自 `src/interfaces/ui/public/`，没有独立的 Desktop 页面副本。

先将 `RECUVORA_UI_BUILD_DIR` 设置为项目根之外的专用构建目录，再从项目根选择目标：

```powershell
./scripts/build-ui.ps1 -Target Web -BuildDir $env:RECUVORA_UI_BUILD_DIR
./scripts/build-ui.ps1 -Target Desktop -BuildDir $env:RECUVORA_UI_BUILD_DIR
./scripts/build-ui.ps1 -Target All -BuildDir $env:RECUVORA_UI_BUILD_DIR
```

Web 构建只需要 Node.js，直接检查并复制零第三方依赖的 HTML、CSS、JavaScript、SVG 和 ICO；不运行 `npm install`，也不在源码目录生成依赖或缓存。Desktop 和 All 还需要 Rust 1.98.1、平台链接器及 Tauri 构建依赖。可用 `-NodePath`、`-CargoPath` 选择已有工具，`-Profile Debug|Release` 选择 Desktop 配置；默认是 `Release`。脚本不会安装工具或修改全局环境。

每次构建先检查共享 JavaScript 语法，再由 [build-ui.mjs](build-ui.mjs) 计算内容标识并写入：

- `<BuildDir>/ui-dist/<build-id>/`：可部署的静态资产；`Web` 与 `All` 在结果中返回该目录。
- `<BuildDir>/ui-manifests/<build-id>.json`：文件大小和 SHA-256 清单。
- `<BuildDir>/cargo/{debug|release}/recuvora-desktop[.exe]`：`Desktop` 与 `All` 的 Tauri 程序。
- `<BuildDir>/cargo/{debug|release}/build/recuvora-*/out/tauri-build-stage/`：Tauri 构建所需配置和 manifest 输入副本，以及 `gen/schemas/` 等自动生成文件。它仅是构建暂存，不是另一个 Cargo 包或维护中的 UI 源。

脚本最后输出一行 JSON，区分本次目标及实际生成的 Web 目录、Desktop 程序和资产清单。`Desktop` 也会校验并暂存共享资产以生成可审查清单，Tauri 本身从同一源码目录嵌入资产。清单递归收集共享目录内的静态文件，新增页面模块不需维护第二套文件列表。所有这些路径都必须在项目根之外；脚本拒绝项目内输出及链接式构建根。

Tauri 2.6 构建辅助默认把 ACL/schema 写入工作目录；[build.rs](../build.rs) 因此在已确认位于项目外的 Cargo `OUT_DIR` 暂存构建输入，再调用官方 `try_build`。原始输入仍由 Cargo 跟踪，Desktop 的 `generate_context!` 继续读取唯一共享前端。新增原生权限、capabilities 或平台专用 Tauri 配置时，必须先扩展暂存输入，不能只忽略源码中的生成文件。

Cargo 仍只有一个 `recuvora` 包。默认 `cli` feature 构建现有 `recuvora` 命令；`desktop-ui` 启用 Tauri 并隐含 `web-ui`，`recuvora-desktop` 只在该 feature 下可用。推荐使用上面的脚本选择前端，因为 Cargo feature 本身不会产生可托管的 Web 输出。

## UI 检查

[check-ui.ps1](check-ui.ps1) 在项目外构建根执行一次 `All`/`Debug` 构建，运行共享资产与安全语义不变量测试，并对 `recuvora-desktop` 运行启用 `desktop-ui` 的 Clippy：

```powershell
./scripts/check-ui.ps1 -BuildDir $env:RECUVORA_UI_BUILD_DIR
```

这项检查验证共享资产可构建、JavaScript 可解析、凭据仅在内存、网络失败保留 Unknown、不自动重试、冲突与安全渲染，以及 Desktop Rust 目标静态检查。HTTP 行为由主检查脚本的 server 测试覆盖；实际浏览器、Tauri 窗口与真实节点端到端验证需分别执行，不能用静态构建代替。

## Windows CI

[Windows workflow](../.github/workflows/windows.yml) 在 `push`、`pull_request` 或手动触发后，使用 `windows-2025`、Rust 1.98.1 MSVC 和 Node.js 24 运行宿主检查与 UI 检查。依赖、Cargo 输出、Web 资产和测试数据均位于 runner 的项目外临时目录；最后检查源码未改变、测试临时目录已清空。工作流不需要外部提供方凭据，不调用外部模型或目标桌面。

Action 固定到已核对官方标签的提交：[checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1)、[setup-node v7.0.0](https://github.com/actions/setup-node/releases/tag/v7.0.0)。运行环境说明见 [GitHub Windows runner 镜像](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)。提交工作流不代表远端 CI 已运行；[实现状态](../docs/implementation-status.md)只记录当前自动验证边界，不保存开发机器验收流水。

模拟任务演示入口及限制见 [启动模块](../src/boot/README.md)。
