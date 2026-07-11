import type {
  AgentRunRecord,
  AgentRunResponse,
  AgentRunResult,
  AgentSpec,
  AgentRuntimeHttpClientOptions,
  AgentTrace,
  ApprovalDecision,
  CancelRunResponse,
  ChatResumeRequest,
  ChatToolCall,
  ChatToolResult,
  ChatTurnEvent,
  ChatTurnState,
  ChatTurnRequest,
  ErrorResponse,
  GenerateObjectRequest,
  GenerateObjectResult,
  HookEvent,
  HookSpec,
  JsonValue,
  LlmCompleteTransport,
  LlmMessage,
  LlmRequest,
  LlmResponse,
  PolicyDecision,
  ProposalActionResponse,
  ProposalDecisionRequest,
  ProposalDiff,
  ProposalEnvelope,
  ProposalWarning,
  ReplayExecutionResponse,
  RunAgentParams,
  RunRequest,
  RunWorkflowParams,
  RuntimeAgentMetrics,
  RuntimeLlmProviderMetrics,
  RuntimeMetricsSummary,
  RuntimeToolMetrics,
  SessionCreateRequest,
  SessionCreateResponse,
  SessionRecord,
  SessionShowResponse,
  StepRecord,
  ThreadForkRequest,
  ThreadForkResponse,
  ThreadRecord,
  ThreadWithSteps,
  ToolSpec,
  TraceEvent,
  TraceSpan,
  WorkflowInputMapping,
  WorkflowRunNode,
  WorkflowRunNodeCompensationResult,
  WorkflowRunNodeResult,
  WorkflowRunRequest,
  WorkflowRunResult,
} from '../src/index.js'

const userMessage = {
  role: 'user',
  content: 'Read task task_1',
  metadata: {},
} as const satisfies LlmMessage

const assistantToolUseMessage = {
  role: 'assistant',
  content: [
    {
      type: 'tool_use',
      id: 'call_1',
      name: 'read_task',
      input: {id: 'task_1'},
    },
  ],
  metadata: {},
} as const satisfies LlmMessage

const readTaskTool = {
  name: 'read_task',
  description: 'Read a task',
  input_schema: {type: 'object'},
  output_schema: {type: 'object'},
  risk: 'read_only',
  metadata: {},
} as const satisfies ToolSpec

const runRequest = {
  protocol_version: 'agent.v1',
  input: {message: 'hello runtime'},
  trigger: 'manual',
  workflow: {
    workflow_id: 'workflow_daily_ops',
    root_run_id: 'run_018f0000-0000-7000-8000-000000000010',
    parent_run_id: 'run_018f0000-0000-7000-8000-000000000011',
    parent_agent_id: 'coordinator_agent',
    dependencies: [
      {
        run_id: 'run_018f0000-0000-7000-8000-000000000012',
        edge: 'requires',
        metadata: {artifact: 'briefing'},
      },
    ],
    fanout_id: 'fanout_research_batch',
    fanin_id: 'fanin_research_batch',
    compensation: {
      compensates_run_id: 'run_018f0000-0000-7000-8000-000000000013',
      strategy: 'rollback',
      metadata: {reason: 'downstream_failure'},
    },
    metadata: {lane: 'research'},
  },
  metadata: {},
} as const satisfies RunRequest

const workflowInputMappings = [
  {
    from_node: 'collect',
    from_path: '/account_id',
    to_path: '/source/account_id',
  },
  {
    from_node: 'collect',
    from_path: '/missing/region',
    to_path: '/source/region',
    transform: 'string',
    default: 'us',
  },
] satisfies WorkflowInputMapping[]

const workflowRunNodes = [
  {
    node_id: 'collect',
    agent_id: 'collector',
    input: {account_id: 'acct_001'},
    compensation: {
      agent_id: 'collector_compensator',
      strategy: 'release_collected_snapshot',
      input: {account_id: 'acct_001'},
      metadata: {scope: 'snapshot'},
    },
    metadata: {phase: 'read'},
  },
  {
    node_id: 'summarize',
    agent_id: 'summarizer',
    input: {format: 'brief'},
    input_mappings: workflowInputMappings,
    depends_on: ['collect'],
    metadata: {phase: 'write'},
  },
] satisfies WorkflowRunNode[]

const workflowRunRequest = {
  protocol_version: 'agent.v1',
  workflow_id: 'workflow_daily_ops',
  user: {
    user_id: 'user_123',
    metadata: {},
  },
  scope: {
    type: 'tenant',
    id: 'tenant_acme',
  },
  trigger: 'queue',
  trigger_envelope: {
    source: 'ops.jobs',
    id: 'msg_001',
    received_at: '2026-06-28T09:12:31Z',
    payload: {job: 'daily_ops'},
    metadata: {},
  },
  nodes: workflowRunNodes,
  metadata: {tenant_id: 'tenant_acme'},
} as const satisfies WorkflowRunRequest

const traceEvents = [
  {
    kind: 'run_started',
    occurred_at: '2026-06-28T09:12:31Z',
    payload: {agent_id: 'echo_agent'},
  },
] satisfies TraceEvent[]

const traceSpans = [
  {
    span_id: 'span_9dbce05c6c23f5ec91d3fbe141e42851d7048b712b5b174c0cf0db2527eafaf4',
    name: 'agent.run',
    started_at: '2026-06-28T09:12:31Z',
    finished_at: '2026-06-28T09:12:32Z',
    duration_ms: 1000,
    status: 'completed',
    attributes: {
      run_id: 'run_018f0000-0000-7000-8000-000000000000',
      agent_id: 'echo_agent',
      status: 'completed',
    },
  },
] satisfies TraceSpan[]

const agentTrace = {
  protocol_version: 'agent.v1',
  runtime_version: '0.1.0',
  run_id: 'run_018f0000-0000-7000-8000-000000000000',
  agent_id: 'echo_agent',
  agent_version: '0.1.0',
  scope: {
    type: 'tenant',
    id: 'tenant_acme',
  },
  started_at: '2026-06-28T09:12:31Z',
  finished_at: '2026-06-28T09:12:32Z',
  input: {message: 'hello runtime'},
  output: {message: 'hello runtime'},
  workflow: {
    workflow_id: 'workflow_daily_ops',
    root_run_id: 'run_018f0000-0000-7000-8000-000000000010',
    parent_run_id: 'run_018f0000-0000-7000-8000-000000000011',
    parent_agent_id: 'coordinator_agent',
    metadata: {lane: 'research'},
  },
  usage_summary: {
    llm_request_count: 1,
    input_tokens: 11,
    output_tokens: 7,
    total_tokens: 18,
    cost_micros_by_currency: {USD: 123},
    by_provider: [
      {
        provider: 'openai',
        model: 'gpt-test',
        request_count: 1,
        input_tokens: 11,
        output_tokens: 7,
        total_tokens: 18,
        cost_micros_by_currency: {USD: 123},
      },
    ],
  },
  artifact_refs: [
    {
      artifact_id: 'artifact_daily_briefing_pdf',
      kind: 'document',
      uri: 'artifact://tenant_acme/daily_briefing.pdf',
      media_type: 'application/pdf',
      size_bytes: 24576,
      sha256: '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
      redaction_classification: 'confidential',
      store: {
        provider: 'host_blob_store',
        bucket: 'tenant-acme-artifacts',
        key: 'daily/2026-06-28/briefing.pdf',
        version: 'v1',
        metadata: {region: 'local'},
      },
      metadata: {title: 'Daily briefing'},
    },
  ],
  spans: traceSpans,
  events: traceEvents,
} as const satisfies AgentTrace

const workflowRunNodeResults = [
  {
    node_id: 'collect',
    agent_id: 'collector',
    status: 'completed',
    run_id: 'run_018f0000-0000-7000-8000-000000000100',
    output: {records: 3},
    trace: agentTrace,
    compensation: {
      agent_id: 'collector_compensator',
      status: 'completed',
      run_id: 'run_018f0000-0000-7000-8000-000000000102',
      output: {released: true},
      metadata: {},
    },
    metadata: {},
  },
] satisfies WorkflowRunNodeResult[]

const workflowRunResult = {
  protocol_version: 'agent.v1',
  workflow_id: 'workflow_daily_ops',
  status: 'completed',
  started_at: '2026-06-28T09:12:31Z',
  finished_at: '2026-06-28T09:12:34Z',
  root_run_id: 'run_018f0000-0000-7000-8000-000000000100',
  nodes: workflowRunNodeResults,
  metadata: {tenant_id: 'tenant_acme'},
} as const satisfies WorkflowRunResult

const agentRunResult = {
  protocol_version: 'agent.v1',
  run_id: 'run_018f0000-0000-7000-8000-000000000000',
  agent_id: 'echo_agent',
  status: 'completed',
  started_at: '2026-06-28T09:12:31Z',
  finished_at: '2026-06-28T09:12:32Z',
  summary: 'echoed input',
  workflow: {
    workflow_id: 'workflow_daily_ops',
    root_run_id: 'run_018f0000-0000-7000-8000-000000000010',
    parent_run_id: 'run_018f0000-0000-7000-8000-000000000011',
    parent_agent_id: 'coordinator_agent',
    metadata: {lane: 'research'},
  },
  output: {message: 'hello runtime'},
} as const satisfies AgentRunResult

const agentRunRecord = {
  protocol_version: 'agent.v1',
  run_id: 'run_018f0000-0000-7000-8000-000000000000',
  idempotency_key: 'idem_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
  agent_id: 'echo_agent',
  status: 'completed',
  scope: {
    type: 'tenant',
    id: 'tenant_acme',
  },
  started_at: '2026-06-28T09:12:31Z',
  finished_at: '2026-06-28T09:12:32Z',
  input: {message: 'hello runtime'},
  output: {message: 'hello runtime'},
  workflow: {
    workflow_id: 'workflow_daily_ops',
    root_run_id: 'run_018f0000-0000-7000-8000-000000000010',
    parent_run_id: 'run_018f0000-0000-7000-8000-000000000011',
    parent_agent_id: 'coordinator_agent',
    metadata: {lane: 'research'},
  },
  metadata: {
    control: {
      cancel_requested: false,
    },
  },
} as const satisfies AgentRunRecord

const agentRunResponse = {
  result: agentRunResult,
  trace: agentTrace,
} as const satisfies AgentRunResponse

const cancelRunResponse = {
  cancellation_requested: true,
  message: 'cancellation intent persisted',
  run_id: 'run_018f0000-0000-7000-8000-000000000000',
  status: 'running',
} as const satisfies CancelRunResponse

const errorResponse = {
  code: 'policy_denied',
  message: 'Policy denied the operation',
  details: {policy_id: 'finance.proposal.default'},
} as const satisfies ErrorResponse

const replayExecutionResponse = {
  source_run_id: 'run_018f0000-0000-7000-8000-000000000000',
  replay_run_id: 'run_018f0000-0000-7000-8000-000000000200',
  agent_id: 'echo_agent',
  mode: 'deterministic',
  output_matches: true,
  result: agentRunResult,
  trace: agentTrace,
} as const satisfies ReplayExecutionResponse

const agentSpec = {
  protocol_version: 'agent.v1',
  id: 'cron_summary',
  name: 'Cron Summary',
  version: '0.1.0',
  schedule: {
    type: 'cron',
    expression: '30 9 * * MON-FRI',
    timezone: 'America/New_York',
  },
  capabilities: ['scheduled_agent'],
  metadata: {domain: 'debug'},
} as const satisfies AgentSpec

const llmRequest = {
  protocol_version: 'agent.v1',
  provider: 'openai-compatible',
  model: 'gpt-5-mini',
  messages: [userMessage],
  response_format: {
    type: 'json_schema',
    name: 'project_summary',
    schema: {
      type: 'object',
      required: ['title', 'status'],
      properties: {
        title: {type: 'string'},
        status: {type: 'string', enum: ['ok', 'blocked']},
      },
      additionalProperties: false,
    },
    strict: true,
  },
  metadata: {prompt_version: 'structured.v1'},
} as const satisfies LlmRequest

const llmResponse = {
  protocol_version: 'agent.v1',
  provider: 'openai-compatible',
  model: 'gpt-5-mini',
  content: '{"title":"Runtime bridge","status":"ok"}',
  finish_reason: 'stop',
  object: {
    title: 'Runtime bridge',
    status: 'ok',
  },
  usage: {
    input_tokens: 10,
    output_tokens: 8,
    total_tokens: 18,
  },
  metadata: {api: 'openai_chat_completions'},
} as const satisfies LlmResponse

const completeTransport = {
  complete(request) {
    void request
    return Promise.resolve(llmResponse)
  },
} satisfies LlmCompleteTransport

const generateObjectRequest = {
  provider: 'openai-compatible',
  model: 'gpt-5-mini',
  messages: [userMessage],
  schemaName: 'project_summary',
  schema: {
    type: 'object',
    required: ['title'],
    properties: {title: {type: 'string'}},
    additionalProperties: false,
  },
  metadata: {prompt_version: 'structured.v1'},
  parse(value) {
    return value as {title: string}
  },
  tools: [readTaskTool],
} as const satisfies GenerateObjectRequest<{title: string}>

const generateObjectResult = {
  object: {title: 'Runtime bridge'},
  request: llmRequest,
  response: llmResponse,
} as const satisfies GenerateObjectResult<{title: string}>

const chatTurnRequest = {
  protocol_version: 'agent.v1',
  turn_id: 'turn_1',
  surface: 'agent_tui',
  mode: 'natural_language',
  session_id: 'session_1',
  thread_id: 'thread_1',
  agent_id: 'ai_chat',
  provider: 'mock',
  model: 'mock-model',
  messages: [userMessage],
  temperature: 0,
  max_output_tokens: 128,
  tools: [readTaskTool],
  metadata: {source: 'contract_test'},
  max_tool_rounds: 4,
} as const satisfies ChatTurnRequest

const pendingToolCalls = [
  {
    id: 'call_1',
    name: 'read_task',
    input: {id: 'task_1'},
  },
] satisfies ChatToolCall[]

const chatTurnState = {
  protocol_version: 'agent.v1',
  turn_id: 'turn_1',
  surface: 'ai_chat',
  mode: 'chat',
  session_id: 'session_1',
  thread_id: 'thread_1',
  agent_id: 'ai_chat',
  provider: 'mock',
  model: 'mock-model',
  messages: [userMessage, assistantToolUseMessage],
  temperature: 0,
  max_output_tokens: 128,
  tools: [readTaskTool],
  metadata: {source: 'contract_test'},
  max_tool_rounds: 4,
  round: 1,
  pending_tool_calls: pendingToolCalls,
  tool_execution: 'client',
} as const satisfies ChatTurnState

const chatToolResult = {
  tool_call_id: 'call_1',
  tool_name: 'read_task',
  output: {title: 'Task title'},
  is_error: false,
} as const satisfies ChatToolResult

const chatResumeRequest = {
  protocol_version: 'agent.v1',
  state: chatTurnState,
  tool_results: [chatToolResult],
} as const satisfies ChatResumeRequest

const chatTurnEvent = {
  kind: 'round_finished',
  content: null,
  response: {
    protocol_version: 'agent.v1',
    provider: 'mock',
    model: 'mock-model',
    content: '',
    finish_reason: 'tool_call',
    usage: {
      input_tokens: 4,
      output_tokens: 2,
      total_tokens: 6,
    },
    metadata: {},
  },
  tool_call_id: null,
  tool_name: null,
  partial_input_json: null,
  tool_input: null,
  tool_output: null,
  usage: {
    input_tokens: 4,
    output_tokens: 2,
    total_tokens: 6,
  },
  round: 1,
  metadata: {
    status: 'requires_tool_results',
    chat_state: chatTurnState,
    tool_calls: pendingToolCalls,
    finish_reason: 'tool_call',
  },
} as const satisfies ChatTurnEvent

const contextSnapshotEvent = {
  kind: 'context_snapshot',
  round: 0,
  metadata: {
    context_snapshot: null,
    compaction: null,
  },
} as const satisfies ChatTurnEvent

const sessionRecord = {
  protocol_version: 'agent.v1',
  session_id: 'session_01975d8c-72f5-7f1e-b111-000000000001',
  title: 'Execution review debug session',
  created_at: '2026-06-28T09:12:31Z',
  updated_at: '2026-06-28T09:12:31Z',
  metadata: {source: 'fixture'},
} as const satisfies SessionRecord

const threadRecord = {
  protocol_version: 'agent.v1',
  thread_id: 'thread_01975d8c-72f5-7f1e-b111-000000000002',
  session_id: 'session_01975d8c-72f5-7f1e-b111-000000000001',
  parent_thread_id: null,
  title: 'Baseline',
  created_at: '2026-06-28T09:12:31Z',
  metadata: {source: 'fixture'},
} as const satisfies ThreadRecord

const stepRecord = {
  protocol_version: 'agent.v1',
  step_id: 'step_01975d8c-72f5-7f1e-b111-000000000003',
  thread_id: 'thread_01975d8c-72f5-7f1e-b111-000000000002',
  kind: 'agent_run',
  run_id: 'run_01975d8c-72f5-7f1e-9b7e-c7ef3e0a1000',
  summary: 'catalog dry-run completed',
  payload: {
    agent_id: 'execution_review',
    status: 'completed',
  },
  created_at: '2026-06-28T09:12:31Z',
} as const satisfies StepRecord

const sessionCreateRequest = {
  title: 'Execution review debug session',
  metadata: {source: 'fixture'},
} as const satisfies SessionCreateRequest

const sessionCreateResponse = {
  session: sessionRecord,
  thread: threadRecord,
} as const satisfies SessionCreateResponse

const threadWithSteps = {
  thread: threadRecord,
  steps: [stepRecord],
} as const satisfies ThreadWithSteps

const sessionShowResponse = {
  session: sessionRecord,
  threads: [threadWithSteps],
} as const satisfies SessionShowResponse

const threadForkRequest = {
  parent_thread_id: 'thread_01975d8c-72f5-7f1e-b111-000000000002',
  title: 'Forked review',
  metadata: {source: 'fixture'},
} as const satisfies ThreadForkRequest

const threadForkResponse = {
  parent_thread_id: 'thread_01975d8c-72f5-7f1e-b111-000000000002',
  session_id: 'session_01975d8c-72f5-7f1e-b111-000000000001',
  thread: threadRecord,
} as const satisfies ThreadForkResponse

const approvalDecision = {
  protocol_version: 'agent.v1',
  proposal_id: 'proposal_01975d8c-72f5-7f1e-9b7e-c7ef3e0a1000',
  decision: 'approve',
  approval_level: 'single_user',
  decided_by: 'user_fixture_reviewer',
  decided_at: '2026-06-28T09:13:31Z',
  comment: 'fixture approval',
} as const satisfies ApprovalDecision

const proposalDiffs = [
  {
    path: '/allocations/0/weight',
    operation: 'replace',
    before: 0.1,
    after: 0.15,
    metadata: {asset: 'AAPL'},
  },
] satisfies ProposalDiff[]

const proposalWarnings = [
  {
    severity: 'warning',
    code: 'allocation_change',
    message: 'Allocation changes require user confirmation',
    metadata: {policy: 'finance.proposal.default'},
  },
] satisfies ProposalWarning[]

const proposalEnvelope = {
  protocol_version: 'agent.v1',
  proposal_id: 'proposal_01975d8c-72f5-7f1e-9b7e-c7ef3e0a1000',
  version: 0,
  run_id: 'run_01975d8c-72f5-7f1e-9b7e-c7ef3e0a1000',
  agent_id: 'execution_review',
  kind: 'fake',
  summary: 'Create a fake proposal for approval testing',
  payload: {value: 7},
  risk: 'medium',
  approval_policy: 'manual',
  approval_required: true,
  required_approval_level: 'single_user',
  required_approver_count: 1,
  approval_decisions: [approvalDecision],
  diffs: proposalDiffs,
  warnings: proposalWarnings,
  policy_id: 'finance.proposal.default',
  policy_version: '2026-06-28',
  status: 'pending_approval',
  created_at: '2026-06-28T09:12:31Z',
  expires_at: '2026-06-29T09:12:31Z',
} as const satisfies ProposalEnvelope

const proposalDecisionRequest = {
  decision: 'approve',
  approval_level: 'single_user',
  decided_by: 'user_fixture_reviewer',
  comment: 'fixture approval',
} as const satisfies ProposalDecisionRequest

const proposalActionResponse = {
  action: 'apply',
  tool: 'propose_fake_apply',
  tool_output: {applied: true},
  proposal: proposalEnvelope,
} as const satisfies ProposalActionResponse

const hookSpec = {
  protocol_version: 'agent.v1',
  event: 'BeforeToolCall',
  kind: 'process',
  name: 'deny_high_risk_tool',
  command: ['policy-hook', '--json'],
  effect: 'policy',
  enabled: true,
  timeout_ms: 1000,
  metadata: {policy_id: 'tool.policy.default'},
} as const satisfies HookSpec

const hookEvent = {
  protocol_version: 'agent.v1',
  hook_event: 'BeforeToolCall',
  hook_kind: 'process',
  hook_name: 'deny_high_risk_tool',
  command: ['policy-hook', '--json'],
  run_id: 'run_018f0000-0000-7000-8000-000000000000',
  agent_id: 'echo_agent',
  status: 'completed',
  started_at: '2026-06-28T09:12:31Z',
  finished_at: '2026-06-28T09:12:31Z',
  duration_ms: 5,
  input: {tool_name: 'propose_fake'},
  output: {decision: 'allow'},
  error: null,
} as const satisfies HookEvent

const policyDecision = {
  decision: 'allow',
  reason: 'read-only tool',
  metadata: {policy_id: 'tool.policy.default'},
} as const satisfies PolicyDecision

const runtimeAgentMetrics = {
  run_count: 2,
  runs_by_status: {completed: 2},
  successful_run_count: 2,
  failed_run_count: 0,
  total_run_latency_ms: 42,
  average_run_latency_ms: 21,
} as const satisfies RuntimeAgentMetrics

const runtimeToolMetrics = {
  tool_call_count: 3,
  failed_tool_call_count: 0,
  total_tool_call_latency_ms: 15,
  average_tool_call_latency_ms: 5,
} as const satisfies RuntimeToolMetrics

const runtimeLlmProviderMetrics = {
  request_count: 1,
  input_tokens: 11,
  output_tokens: 7,
  total_tokens: 18,
  total_latency_ms: 125,
  average_latency_ms: 125,
  cost_micros_by_currency: {USD: 123},
} as const satisfies RuntimeLlmProviderMetrics

const runtimeMetricsSummary = {
  protocol_version: 'agent.v1',
  runtime_version: '0.1.0',
  generated_at: '2026-06-28T09:12:31Z',
  store_root: '/tmp/agent-runtime-store',
  run_count: 2,
  runs_by_status: {completed: 2},
  successful_run_count: 2,
  skipped_run_count: 0,
  failed_run_count: 0,
  timeout_count: 0,
  total_run_latency_ms: 42,
  average_run_latency_ms: 21,
  tool_call_count: 3,
  failed_tool_call_count: 0,
  total_tool_call_latency_ms: 15,
  average_tool_call_latency_ms: 5,
  replay_count: 1,
  proposal_count: 1,
  proposals_by_status: {pending_approval: 1},
  proposal_created_count: 1,
  proposal_approved_count: 0,
  proposal_denied_count: 0,
  proposal_applied_count: 0,
  artifact_ref_count: 1,
  llm_total_tokens: 18,
  runs_by_agent: {echo_agent: runtimeAgentMetrics},
  tool_calls_by_tool: {read_task: runtimeToolMetrics},
  llm_usage_by_provider: {openai: runtimeLlmProviderMetrics},
} as const satisfies RuntimeMetricsSummary

const httpClientOptions = {
  baseUrl: 'http://127.0.0.1:8765',
  fetch,
} satisfies AgentRuntimeHttpClientOptions

const runAgentParams = {
  input: {message: 'hello runtime'},
  metadata: {source: 'type_conformance'},
  runId: 'run_018f0000-0000-7000-8000-000000000000',
  sessionId: 'session_1',
  threadId: 'thread_1',
  trigger: 'webhook',
  triggerEnvelope: {
    source: 'github',
    id: 'delivery_1',
    payload: {action: 'opened'},
    metadata: {},
  },
  scope: {type: 'tenant', id: 'tenant_acme'},
  user: {user_id: 'user_123', metadata: {}},
  workflow: {
    workflow_id: 'workflow_daily_ops',
    metadata: {lane: 'research'},
  },
} as const satisfies RunAgentParams

const runWorkflowParams = {
  workflow_id: 'workflow_daily_ops',
  protocol_version: 'agent.v1',
  nodes: workflowRunNodes,
  metadata: {tenant_id: 'tenant_acme'},
} satisfies RunWorkflowParams

void runRequest
void agentRunRecord
void agentRunResponse
void cancelRunResponse
void errorResponse
void replayExecutionResponse
void workflowRunRequest
void workflowRunResult
void agentRunResult
void agentSpec
void llmRequest
void llmResponse
void completeTransport
void generateObjectRequest
void generateObjectResult
void chatTurnRequest
void chatResumeRequest
void chatTurnEvent
void contextSnapshotEvent
void sessionCreateRequest
void sessionCreateResponse
void sessionShowResponse
void threadForkRequest
void threadForkResponse
void proposalDecisionRequest
void proposalActionResponse
void hookSpec
void hookEvent
void policyDecision
void runtimeMetricsSummary
void httpClientOptions
void runAgentParams
void runWorkflowParams
