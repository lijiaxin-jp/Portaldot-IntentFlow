# P6–P8 本地代码交付与验收边界

本阶段补齐执行证据落盘、只读 Dashboard 和公开验收报告生成器。它们可在本地准备好，但这不代表已完成测试网验收。

## P6 执行与证据

- `EnvironmentSr25519Signer` 从 `PORTALDOT_SIGNER_URI` 读取 Secret URI，错误信息不会包含密钥内容。签名发生在 Subxt 构造交易签名载荷之后，签名器不接触 RPC 或交易提交。
- 提交入口只构造 metadata 中的 `Contracts.call`，origin 必须与签名账户相同。执行入口要求策略启用，并在配置要求时校验同一请求、同一区块上的成功 dry-run。
- `SqliteEvidenceStore` 保存 intent、route、quote、模拟、receipt、extrinsic index、事件原文和失败原因。事件按 finalized receipt 中的 extrinsic index 归属，因此包含成功和 `System::ExtrinsicFailed` 结果。
- 默认 `config/portaldot-testnet.toml` 继续关闭执行。此轮没有读取密钥环境变量，没有调用签名器，也没有提交交易。

数据库路径可通过 `INTENTFLOW_EVIDENCE_DB` 设置，默认是 `data/intentflow.sqlite3`。证据中的原始事件字节对本地运维有用；公开包只输出链上事件字段和十六进制数据，不导出签名载荷、Secret URI、RPC URL 或数据库连接串。

## P7 Dashboard

从仓库根目录启动：

```text
npm run p7:dashboard
```

默认地址为 `http://127.0.0.1:8787`。通过 `PORT` 修改端口，通过 `INTENTFLOW_EVIDENCE_DB` 选择证据库。服务只监听 loopback，并以只读方式打开 SQLite。Runtime 和池数据只代表已持久化快照；空库显示未采集，不会伪装成当前链状态。

## P8 公开验收包

从仓库根目录运行：

```text
npm run p8:acceptance
```

生成目录默认为 `artifacts/p8-public-acceptance`，包括 JSON 报告、Markdown 摘要和报告 SHA-256 manifest。参数 `--database PATH`、`--output DIR` 和 `--deployments JSON` 可替换输入或输出位置。部署清单只收录地址、代码 hash、metadata hash、部署交易 hash 和区块 hash。

报告用真实 SQLite receipt 统计交易；至少 10 笔成功的 Portaldot testnet finalized 交易、已核验部署清单、目标网络完整 Contracts 能力探测、dry-run/提交测量样本，以及带 runtime、区块范围和机器配置的运行记录全部齐备后，状态才会是 `ready_for_review`。失败的 finalized 记录仍会保留在报告中，但不会计入 10 笔成功交易。否则报告仍会生成，并列出具体缺项。性能统计包含样本量、最小值、平均值、P50、P95 和最大值。

只读节点探测需要设置 `PORTALDOT_TESTNET_WS_URL`，例如 PowerShell：

```powershell
$env:PORTALDOT_TESTNET_WS_URL = 'wss://test-node-v3.portaldot.dev'
npm run p8:probe
npm run p8:acceptance -- --deployments evidence/p3/p2-deployment.json --probe artifacts/p8-public-acceptance/probe-report.json
```

探测会读取链名、genesis hash、finalized 区块、runtime、metadata 和 Contracts 能力，不需要账户或密钥，不签名、不广播。输出只记录节点主机名，不保存完整 RPC URL。

公开验收仍需人工在节点浏览器核实地址、交易 hash、区块与事件，并记录可复现的主机配置。报告生成器不会创建链上证据，也不会把本地模拟当作测试网交易。
