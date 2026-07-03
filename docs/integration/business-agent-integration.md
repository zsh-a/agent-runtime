# Business Agent Integration Guide

本文档总结其他业务如何基于当前 `agent-runtime` 开发自己的 agent。核心原则是：业务 agent 通常由业务方开发，runtime 提供通用执行内核、协议、调试和追踪能力。

## 架构定位

当前 runtime 是独立 Rust workspace，负责通用 agent 基础设施：

- JSON-first wire contracts and schema validation
- `RunRequest` / `AgentRunResult` / `AgentTrace`
- `ChatTurnRequest` / `ChatTurnEvent`
- tool calling protocol
- proposal and approval envelopes
- LLM provider abstraction
- run lifecycle, retry, timeout, lease, store
- CLI, HTTP, stdio, TUI, replay, eval, debug bundle

业务系统负责业务能力和副作用：

- 业务 agent 的目标、流程和 prompt
- 业务 tool 名称、输入输出 schema、风险等级
- tool 的具体实现
- 用户、账号、租户、权限、feature flag
- 数据库读写、平台 API、外部服务调用
- proposal 的展示、确认、apply、undo
- trace 脱敏和生产级持久化

Runtime 不应该直接 import 业务模型、Flutter/Riverpod、后端框架、数据库 repository 或设备 API。业务能力通过 `AgentServices`、ToolHost、proposal applier 或 HTTP/stdio/FRB bridge 暴露给 runtime。

## 业务 Agent 是否要自己开发

是。业务 agent 的“业务判断”和“业务动作”需要业务方开发，但不需要重做 runtime。

```text
Business Agent = business goal + prompt + tool set + decision flow + proposal policy
Agent Runtime  = protocol + execution + tracing + replay + provider/tool loop
```

优先从配置型 agent 开始：

- 用 catalog 描述 agent、tools、proposal kinds、prompt blocks。
- 把业务能力包装成 JSON tools。
- 通过 `RunRequest` 或 `ChatTurnRequest` 驱动。
- 先接入只读、低风险场景。

当业务流程需要强控制、可重复、可审计时，再实现代码型 agent，例如 Rust `Agent` trait 或宿主应用自己的 typed adapter。

## 推荐接入层

业务应用建议在自己的仓库中增加一层 agent runtime adapter，而不是让业务 feature 直接调用底层 runtime。

```text
business-app/
  agent_runtime/
    catalog.*
    agent_runtime_bridge.*
    agent_runtime_tool_host.*
    agent_runtime_proposals.*
    agent_runtime_trace_recorder.*
  domain/
    services/
    repositories/
```

Feature 代码依赖业务语义接口，例如 `CustomerSummaryAgent`、`PortfolioBriefingAgent`、`AiChatRunner`，不要直接依赖 HTTP route、FRB function 或 JSON-RPC method。

## 开发流程

1. 选择一个低风险业务场景。

   例如客户摘要、订单归因、客服草稿、投资组合摘要、收件箱分类。

2. 定义 agent。

   在 catalog 中定义 `id`、`name`、`version`、`schedule`、`capabilities` 和 `metadata`。业务 prompt 放在 `prompt_blocks` 或由业务侧生成 prompt manifest。

3. 定义 tools。

   Tool 名称使用稳定 snake_case。输入输出保持 JSON object shape。只读 tool 标记为 `read_only`，写入、交易、通知、删除、健康数据等标记为 `medium` 或 `high`。

4. 实现 ToolHost。

   ToolHost 接收 `{ name, input, context }`，分发到业务 service/repository/platform API，并返回 JSON object。Runtime 不直接访问业务 repository。

5. 选择运行入口。

   - 后端/Web/多客户端共享 runtime：HTTP server
   - Rust 后端内嵌：直接依赖 crates 并实现 `AgentServices`
   - Flutter/手机端：FRB native bridge，Dart 实现 ToolHost
   - CLI/IDE/插件：stdio JSON-RPC

6. 接入 proposal。

   对用户可见副作用，优先让 agent 生成 `ProposalEnvelope`。宿主应用展示 summary、diff、warning，用户确认后由业务 proposal applier 执行。

7. 接入 trace。

   Runtime 输出 `AgentTrace` 和 chat events。业务侧负责脱敏、关联 user/session/thread，并写入生产 trace store。

8. 加 contract tests。

   至少覆盖 catalog schema、tool schema、tool golden request/response、proposal confirmation、trace redaction、provider profile secret leakage。
   对 catalog dry-run 集成，可以先用 `agent compat check --catalog ... --tool-source ... --run-input ... --proposal-input ...`
   做 CI smoke test，再补业务 repository 自己的更细测试。

## RunRequest 和 ChatTurnRequest 的选择

使用 `RunRequest`：

- 后台任务
- 定时摘要
- 明确 workflow
- 分类、归因、生成报告
- proposal 生成
- 需要 run trace/replay 的任务

使用 `ChatTurnRequest`：

- 用户聊天
- 流式 assistant response
- 多轮 tool calling
- `ask_user` 暂停/恢复
- 移动端或 Web chat UI

一个业务通常两者都会用：聊天入口走 `ChatTurnRequest`，后台 job 和可审计 workflow 走 `RunRequest`。

## 接入方式

### HTTP server

适合 Web、后端服务、多个客户端共享一个 runtime 进程。

```bash
cargo run -p agent-cli -- serve \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --store /private/tmp/agent-runtime-business-example-store
```

常用接口：

- `GET /catalog/summary`
- `GET /tools`
- `POST /agents/{agent_id}/run`
- `GET /runs/{run_id}/trace`
- `GET /runs/{run_id}/events`
- `GET /proposals`
- `POST /proposals/{proposal_id}/decision`
- `POST /proposals/{proposal_id}/apply`

### Rust crate

适合 Rust 后端内嵌。业务方实现：

- `Agent`
- `AgentServices`
- `AgentRunStore`
- `AgentProposalStore`
- 可选 `AgentLockStore`、`AgentSessionStore`

这种方式没有 HTTP sidecar，部署简单，但业务进程需要承担 runtime 生命周期。

### Flutter / mobile

适合本地执行、直接使用设备凭据和平台 API 的手机 App。

推荐边界：

```text
Flutter UI
  -> Feature-owned Dart agent adapter
  -> App-level AgentRuntimeBridge
  -> FRB native Rust bridge
  -> agent-chat / agent-runtime / agent-llm
  -> Dart ToolHost
  -> Riverpod providers / repositories / platform APIs
```

Dart 负责权限、业务状态、数据库写入、设备 API 和用户确认。Rust 负责协议校验、provider mapping、ChatTurn state、run/proposal envelopes 和 trace。

### stdio

适合本地插件、IDE、测试 harness。当前 stdio server 使用 JSON-RPC 风格 envelope，一行一个请求，支持 `catalog.summary` 和 `agent.run`。

## 最小业务例子

仓库内提供了一个客户摘要示例：

- `examples/business-integration/catalog.json`
- `examples/business-integration/tool-source.json`
- `examples/business-integration/customer_tool_host.py`
- `examples/business-integration/run-customer-summary.json`
- `examples/business-integration/run-followup-proposal.json`

校验合约：

```bash
cargo run -p agent-cli -- validate \
  schemas/catalog.schema.json \
  examples/business-integration/catalog.json

cargo run -p agent-cli -- validate \
  schemas/tool-source-manifest.schema.json \
  examples/business-integration/tool-source.json
```

运行一次 dry-run tool call：

```bash
cargo run -p agent-cli -- run customer_summary_agent \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --input examples/business-integration/run-customer-summary.json \
  --store /private/tmp/agent-runtime-business-example-store
```

生成一个 proposal envelope：

```bash
cargo run -p agent-cli -- run customer_summary_agent \
  --catalog examples/business-integration/catalog.json \
  --tool-source examples/business-integration/tool-source.json \
  --input examples/business-integration/run-followup-proposal.json \
  --store /private/tmp/agent-runtime-business-example-store
```

这个示例使用 catalog dry-run agent，因此它验证 runtime 生命周期、tool dispatch、proposal envelope、trace store。真实业务接入时，把 `customer_tool_host.py` 替换为业务自己的 ToolHost，并把 dry-run agent 替换为配置型 chat/run adapter 或代码型业务 agent。

## 设计边界清单

放在业务侧：

- customer/account/tenant context
- permission checks
- feature flags
- business prompt policy
- tool implementation
- proposal confirmation and apply
- production trace persistence
- secret management

放在 runtime：

- stable JSON contracts
- run/chat lifecycle
- LLM provider abstraction
- tool round orchestration
- retry, timeout, lease
- trace, replay, eval
- CLI, HTTP, stdio developer surfaces

只有当需求是通用协议、runner 生命周期、LLM provider、store/transport 或通用 tool adapter 时，才应该修改 runtime 仓库。
