# 审批与受限文本修复

当前已实现指定 Harness 执行、独立 Harness 审批、人工决定、持久执行许可和 Windows 本机受控文本替换。执行与审批可以使用同一个命名实例、模型及账号，但每次审批使用新的 Hidden 会话和无工具上下文。此能力可由 `repair` CLI 或[官方控制台 API](console-api.md)使用；原有 `demo` 保持无副作用模拟，`harness run` 保持纯文本调用。

## 独立节点与控制台

执行模型的上下文、审批模型的上下文与实际文件目标分别配置。repair 配置通过 `extensions_config` 连接节点，并以 `execution_workspace: {"node_id":"harness-node","workspace_id":"work"}` 选择执行上下文；远端审批另行配置 `reviewer_workspace`。节点把工作区 ID 映射到自己的本机目录，宿主不会对远端路径做本地 canonicalize。`target_root` 仍是宿主当前受限文件执行器的实际目标，不能把它当作模型工作区；Linux 宿主的真实文件动作仍未实现。

宿主只通过通用 `remote-node` 代理接入 Harness。独立节点管理具体提供方进程、凭据环境与协议兼容，不依赖完整宿主 crate 或 UI。

控制台请求使用服务端令牌认证，审计 actor 来自服务端 operator 配置。批准、拒绝、撤销、执行和消歧必须携带当前 revision，在存储锁内检查后再改变权威状态；过期页面返回 409。批准不会隐式执行，模型工具不可调用人工决定接口。单操作员令牌并非多用户身份系统。

## 运行流程

1. 本机操作员显式加载项目外的配置，提供任务请求、目标、允许文件和委托审批政策。
2. 执行 Harness 使用宿主提供的 `recuvora_read_text` 读取白名单文件，使用 `recuvora_replace_text` 提议全文替换。工具参数包括相对路径、完整预期原文和完整替换内容。
3. 宿主检查文件及预期原文，持久保存具体操作、原始用户请求、执行会话关联和政策快照。模型不能调用人工决定或修改授权存储。
4. 审批 Harness 接收准确操作、用户请求与委托政策，只返回 `request_id`、`decision` 和 `reason`。来源 Harness 和会话标识由宿主从调用结果取得，不接受模型自行填报。决定为 `approve`、`deny` 或 `escalate`。
5. 宿主确认操作在硬性目标/动作范围内、策略未变化、请求未过期或撤销。文件执行端重新核对原文及对象身份，持久记录 `Executing` 后只执行一次，写入同步落盘并读回验证，然后记录结果。
6. 拒绝、待人工、无效回复、过期或未知结果阻止本轮后续工具派发。工具调用失败也会反映在最终状态中，模型正常结束不能覆盖工具失败。

批准文本不会直接成为权限；`ExecutionPermit` 只能由可信存储产生，不能从 JSON 反序列化、复制或跨存储使用。目标内容变化使当前批准失效，需重新提交准确操作。策略变更需提升版本，完整政策也会参与一致性比较。

## 配置与启动

使用 [repair 配置示例](../profiles/repair.local.example.json)，复制到项目外，并设置 `RECUVORA_REPAIR_CONFIG` 指向其活动副本。配置路径均相对于该配置文件解析：

| 字段 | 含义 |
| --- | --- |
| `harness_config` | 已实现的 Harness registry schema v1 活动配置，同样必须位于源码和修复目标外 |
| `extensions_config` | 已安装节点的受信连接配置，必须位于源码和修复目标外 |
| `execution_workspace`、`reviewer_workspace` | 执行与审批使用的节点 ID、工作区 ID；都必须属于相应 Harness 配置允许范围 |
| `execution_harness` | 明确选择执行实例，不使用失败回退 |
| `target_id`、`target_root` | 目标业务标签及实际目录；审批同时保存实际根，改标签不能绕过同根未知结果阻断 |
| `allowed_files` | 1 至 64 个准确相对文件名，使用 `/`；无通配符，不授权整个目录树 |
| `reviewer_directory` | 已存在、位于目标外的可信审批上下文目录；建议使用专用空目录 |
| `data_dir` | 稳定、项目与目标之外的授权记录目录；不可为绕过未决记录而更换 |
| `policy.reviewer` | `{"mode":"human"}` 或 `{"mode":"harness","harness_id":"harness-external"}` |
| `policy.delegation` | 本机操作员提供的明确 AI 审批委托；未覆盖的操作应升级人工 |
| `policy.allowed_targets`、`policy.allowed_action_kinds` | 硬性精确匹配范围，人工也不能越过；当前动作仅 `replace_text` |
| `policy.version`、`policy.ttl_secs` | 正整数政策版本，以及从请求创建开始计算的 1 至 86400 秒有效期 |
| `timeout_secs`、`max_tool_calls` | 本轮 1 至 1800 秒、1 至 64 次工具调用上限；评审仍受本轮取消和自身期限限制 |

节点工作区 ID 须包含在相应 Harness 的 `workspace_roots` 中；使用同一实例时，该实例必须同时允许执行与审批工作区。`target_root`、`allowed_files` 仍由宿主动作执行器独立核验，不能由节点工作区扩大。配置、状态和本地审批目录不得位于修复目标内；Recuvora 源码、程序安装目录、控制目录和活动配置受执行端保护。目标副本及白名单文件须预先存在，状态目录可由 CLI 创建。

```powershell
cargo run --locked -- repair run --config $env:RECUVORA_REPAIR_CONFIG --task repair-001 --prompt "将 settings.txt 中指定的旧配置改为新配置"
cargo run --locked -- repair inspect --config $env:RECUVORA_REPAIR_CONFIG
```

`run` 使用配置指定的独立节点和工作区。执行会话为 Hidden，不请求原生项目归组；这只控制客户端可见性，不代表模型离线，也不改变提供方的数据发送路径。宿主不会自动创建项目、安装节点或修改提供方的用户配置。

## 人工接管

人工策略、评审不确定、失效或结构不合规时保留待处理请求。先用 `inspect --request ID` 查看完整原文、替换、目标、政策、评审理由与当前状态，然后按具体请求操作：

```powershell
cargo run --locked -- repair approve --config $env:RECUVORA_REPAIR_CONFIG --request approval-0000000000000001 --reason "已审查该请求的准确变更"
cargo run --locked -- repair apply --config $env:RECUVORA_REPAIR_CONFIG --request approval-0000000000000001
```

示例 ID 需替换为 `inspect` 返回值。`approve` 只记录批准，`apply` 重新检查并执行这一条已存操作；`deny` 和 `revoke` 均要求 `--request` 与 `--reason`。撤销、取消和过期的请求不能复活。批准可在重启后显式消费，但不会在启动时自动执行。已有持久操作的任务 ID 不能通过再次 `run` 重放。

这是本机单操作员 CLI，以操作系统账号及状态文件权限作为管理边界；记录中的 `local-cli-operator` 是来源标记，不是远程身份认证。尚无远程审批、GUI 或多用户认证服务。运行期间进程持有状态目录写锁，另一进程不能同时审批或撤销；先 Ctrl-C 等待当前进程收尾，再使用管理命令。库内取消和受控撤销保留相同业务约束。

## 执行与恢复

当前动作仅支持 Windows 本机磁盘上既有、单链接、非 reparse point 的 UTF-8 普通文件，每个文件最大 16 KiB。执行保留目录和文件句柄，并核对实际句柄对应路径，拒绝路径穿越、ADS、设备路径、链接及未列入文件。它不提供 shell、文件创建/删除、发布、服务重启或桌面输入。

文件执行会再次检查完整原文，阻止普通并发写入和重命名，并在最后检查硬链接数。Windows 共享模式不能禁止任意同账号进程随时创建硬链接；当前实现不是对恶意同账号程序的操作系统沙箱。高信任隔离部署仍需独立运行身份和文件权限；同进程可信代码也不受 Rust 服务接口约束。

全文替换采用原文件写入，并非事务性原子替换。进程或磁盘在写入中失效，目标可能留下部分内容，状态必须保持 `Unknown`。准确原文和替换内容已在执行意图之前持久保存，允许操作员核验；不会自动覆盖或回退。

授权日志为独立的有界 JSONL，默认最多 10000 个请求及 32 MiB，持有单写入者锁。请求、评审、人工决定、授权消费及结果分别落盘。完整记录损坏或尾部残片均拒绝启动，不通过截断可能的执行意图恢复旧批准。容量耗尽也拒绝继续派发。

重启遇到 `Executing` 会落盘为 `Unknown` 并阻断同一目标的新动作。执行已完成但结果日志失败时返回 `unknown`，不能解释为未执行。显式运行以下命令只读取现状，不重复写入：

```powershell
cargo run --locked -- repair reconcile --config $env:RECUVORA_REPAIR_CONFIG --request approval-0000000000000001
```

现状等于替换内容时记为 `Executed`，等于原文时记为 `Failed`；两者都不匹配则保持未知，交给操作员处理。检查的是当前文件状态，不证明历史上从未发生过短暂写入。记录查看、拒绝及撤销不依赖目标文件仍然存在。

输出为 JSON，外部字符串经过编码转义。退出码：`0` 为命令完成，`1` 为失败/阻断，`2` 为参数错误，`3` 为未知结果，`4` 为待人工，`130` 为取消。批准或查看命令成功不代表执行；应查看具体记录状态。运行不会自动重试；`run` 的流程报告及 `apply` 返回的动作结果包含 `auto_retry=false` 和 `business_verified=false`，明确文件读回不是业务恢复证明。配置或调用错误返回独立错误对象，管理命令返回记录，两者不保证包含业务核验字段。目标专属业务验证仍未接入。

## 提供方关系及验证

Recuvora 自行维护委托政策、权威决定和一次执行许可，不启用或信任提供方私有的自动审批作为业务授权。节点只能转发宿主显式登记的工具请求；原生命令、提供方文件修改和原生审批通道保持关闭。执行和审批使用独立会话及容量，避免执行占满后无法评审。取消或协议失效后，宿主仍等待已经派发的可信回调记录最终结果，再结束代理调用。

自动测试使用协议夹具、内存 Harness 和外部隔离文件，验证同实例评审与执行、拒绝/升级、人工批准后重启执行、一次消费、政策/目标变化、取消、日志失败、未知核验及 Windows 路径边界。默认测试不调用外部模型；提供方集成、其他平台和长期运行由独立节点及部署验收分别覆盖。参见[测试说明](../tests/README.md)。
