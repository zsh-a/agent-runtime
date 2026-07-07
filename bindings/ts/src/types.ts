export type ProtocolVersion = 'agent.v1'

export type JsonPrimitive = boolean | null | number | string
export type JsonValue = JsonPrimitive | readonly JsonValue[] | {[key: string]: JsonValue | undefined}
export type JsonObject = {[key: string]: JsonValue | undefined}

export type TriggerKind = 'manual' | 'queue' | 'replay' | 'scheduled' | 'webhook'
export type AgentRunStatus = 'abandoned' | 'cancelled' | 'completed' | 'failed' | 'running' | 'skipped' | 'timed_out'
export type ToolRisk = 'high' | 'low' | 'medium' | 'read_only'
export type ApprovalLevel = 'admin' | 'multi_approver' | 'none' | 'single_user'
export type ReplayMode = 'deterministic' | 'live' | 'view'
export type ArtifactKind = 'blob' | 'dataset' | 'document' | 'image' | 'log' | 'other'
export type RedactionClassification = 'confidential' | 'internal' | 'public' | 'secret'
export type StepKind = 'agent_run' | 'approval' | 'llm_round' | 'note' | 'proposal' | 'state_update' | 'tool_call'
export type ProposalDiffOperation = 'add' | 'remove' | 'replace'
export type ProposalWarningSeverity = 'danger' | 'info' | 'warning'
export type LlmRole = 'assistant' | 'system' | 'tool' | 'user'
export type LlmFinishReason = 'content_filter' | 'error' | 'length' | 'stop' | 'tool_call'
export type ChatToolExecution = 'client' | 'runtime'
export type ContextBlockKind =
  | 'agent_instructions'
  | 'command_instructions'
  | 'compaction_summary'
  | 'memory'
  | 'message'
  | 'metadata'
  | 'resource'
  | 'runtime_instructions'
  | 'tool_schema'
export type HookEventName =
  | 'AfterAgentStep'
  | 'AfterCompact'
  | 'AfterProposalDecision'
  | 'AfterStateSave'
  | 'AfterToolCall'
  | 'BeforeAgentStep'
  | 'BeforeCompact'
  | 'BeforeProposalApply'
  | 'BeforeProposalCreate'
  | 'BeforeStateSave'
  | 'BeforeToolCall'
  | 'RunStart'
  | 'RunStop'
  | 'SessionStart'
  | 'SessionStop'
  | 'SubagentStart'
  | 'SubagentStop'
export type HookKind = 'native_rust' | 'process' | 'server'
export type HookEffect = 'observe' | 'policy'
export type HookInvocationStatus = 'completed' | 'failed'
export type PolicyDecisionKind = 'allow' | 'deny'

export interface UserContext {
  metadata?: JsonObject
  user_id: string
}

export type ScheduleSpec =
  | {type: 'manual'}
  | {every_seconds: number; jitter_seconds?: number | null; preferred_hour_local?: number | null; type: 'interval'}
  | {expression: string; timezone: string; type: 'cron'}

export interface AgentSpec {
  capabilities?: string[]
  description?: null | string
  id: string
  metadata?: JsonObject
  name: string
  protocol_version: ProtocolVersion
  schedule: ScheduleSpec
  version: string
}

export interface ToolSpec {
  description: string
  input_schema: JsonObject
  metadata?: JsonObject
  name: string
  output_schema?: JsonObject | null
  risk: ToolRisk
}

export interface RunRequest {
  input?: JsonObject
  metadata?: JsonObject
  protocol_version: ProtocolVersion
  run_id?: string | null
  scope?: null | RunScope
  trigger?: TriggerKind
  trigger_envelope?: null | TriggerEnvelope
  user?: UserContext | null
  workflow?: null | RunWorkflow
}

export type RunScope =
  | {type: 'global'}
  | {id: string; type: 'user'}
  | {id: string; type: 'tenant'}

export interface RunDependency {
  edge?: null | string
  metadata?: JsonObject
  run_id: string
}

export interface RunCompensation {
  compensates_run_id: string
  metadata?: JsonObject
  strategy?: null | string
}

export interface RunWorkflow {
  compensation?: null | RunCompensation
  dependencies?: RunDependency[]
  fanin_id?: null | string
  fanout_id?: null | string
  metadata?: JsonObject
  parent_agent_id?: null | string
  parent_run_id?: null | string
  root_run_id?: null | string
  workflow_id?: null | string
}

export interface WorkflowRunNode {
  agent_id: string
  compensation?: null | WorkflowRunNodeCompensation
  depends_on?: string[]
  input?: JsonObject
  input_mappings?: WorkflowInputMapping[]
  metadata?: JsonObject
  node_id: string
  run_id?: null | string
}

export interface WorkflowInputMapping {
  default?: JsonValue
  from_node: string
  from_path?: string
  to_path: string
  transform?: WorkflowInputTransform
}

export type WorkflowInputTransform =
  | 'none'
  | 'string'
  | 'number'
  | 'integer'
  | 'boolean'
  | 'json_string'

export interface WorkflowRunNodeCompensation {
  agent_id: string
  input?: JsonObject
  metadata?: JsonObject
  run_id?: null | string
  strategy?: null | string
}

export interface WorkflowRunRequest {
  metadata?: JsonObject
  nodes?: WorkflowRunNode[]
  protocol_version: ProtocolVersion
  root_run_id?: null | string
  scope?: null | RunScope
  trigger?: TriggerKind
  trigger_envelope?: null | TriggerEnvelope
  user?: null | UserContext
  workflow_id: string
}

export interface WorkflowRunNodeResult {
  agent_id: string
  compensation?: null | WorkflowRunNodeCompensationResult
  depends_on?: string[]
  error?: null | AgentErrorRecord
  metadata?: JsonObject
  node_id: string
  output?: JsonValue
  run_id?: null | string
  status: AgentRunStatus
  trace?: null | AgentTrace
}

export interface WorkflowRunNodeCompensationResult {
  agent_id: string
  error?: null | AgentErrorRecord
  metadata?: JsonObject
  output?: JsonValue
  run_id?: null | string
  status: AgentRunStatus
  trace?: null | AgentTrace
}

export interface WorkflowRunResult {
  finished_at: string
  metadata?: JsonObject
  nodes?: WorkflowRunNodeResult[]
  protocol_version: ProtocolVersion
  root_run_id?: null | string
  started_at: string
  status: AgentRunStatus
  workflow_id: string
}

export interface TriggerEnvelope {
  id?: null | string
  metadata?: JsonObject
  payload?: JsonValue
  received_at?: null | string
  source: string
}

export interface AgentRunRecord {
  agent_id: string
  error?: AgentErrorRecord | null
  finished_at?: null | string
  idempotency_key?: null | string
  input?: JsonObject
  metadata?: JsonObject
  output?: JsonObject
  protocol_version: ProtocolVersion
  run_id: string
  scope: RunScope
  started_at: string
  status: AgentRunStatus
  workflow?: null | RunWorkflow
}

export interface AgentRunResult {
  agent_id: string
  error?: AgentErrorRecord | null
  finished_at: string
  output?: JsonObject
  protocol_version: ProtocolVersion
  run_id: string
  started_at: string
  status: AgentRunStatus
  summary?: null | string
  workflow?: null | RunWorkflow
}

export interface CancelRunResponse {
  cancellation_requested: boolean
  message: string
  run_id: string
  status?: AgentRunStatus
}

export interface AgentErrorRecord {
  code: string
  details?: JsonObject
  kind: string
  message: string
  retryable: boolean
}

export interface ErrorResponse {
  code: string
  details?: JsonObject
  message: string
}

export interface TraceEvent {
  kind: string
  occurred_at: string
  payload?: JsonObject
}

export interface ArtifactRef {
  artifact_id: string
  kind?: ArtifactKind
  media_type?: null | string
  metadata?: JsonObject
  redaction_classification?: RedactionClassification
  sha256?: null | string
  size_bytes?: null | number
  store?: null | ArtifactStoreRef
  uri: string
}

export interface ArtifactStoreRef {
  bucket?: null | string
  key?: null | string
  metadata?: JsonObject
  provider: string
  version?: null | string
}

export interface TraceSpan {
  attributes?: JsonObject
  duration_ms: number
  finished_at: string
  name: string
  parent_span_id?: null | string
  span_id: string
  started_at: string
  status: string
}

export interface TraceUsageProviderSummary {
  cost_micros_by_currency?: Record<string, number>
  input_tokens: number
  model?: null | string
  output_tokens: number
  provider: string
  request_count: number
  total_tokens: number
}

export interface TraceUsageSummary {
  by_provider?: TraceUsageProviderSummary[]
  cost_micros_by_currency?: Record<string, number>
  input_tokens: number
  llm_request_count: number
  output_tokens: number
  total_tokens: number
}

export interface AgentTrace {
  agent_id: string
  agent_version: string
  artifact_refs?: ArtifactRef[]
  events: TraceEvent[]
  finished_at: string
  input?: JsonObject
  output?: JsonObject
  protocol_version: ProtocolVersion
  run_id: string
  runtime_version: string
  scope: RunScope
  spans?: TraceSpan[]
  started_at: string
  usage_summary?: null | TraceUsageSummary
  workflow?: null | RunWorkflow
}

export interface ReplayExecutionResponse {
  agent_id: string
  mode: ReplayMode
  output_matches: boolean
  replay_run_id: string
  result: AgentRunResult
  source_run_id: string
  trace: AgentTrace
}

export interface SessionRecord {
  created_at: string
  metadata?: JsonObject
  protocol_version: ProtocolVersion
  session_id: string
  title: string
  updated_at: string
}

export interface ThreadRecord {
  created_at: string
  metadata?: JsonObject
  parent_thread_id?: null | string
  protocol_version: ProtocolVersion
  session_id: string
  thread_id: string
  title?: null | string
}

export interface StepRecord {
  created_at: string
  kind: StepKind
  payload?: JsonObject
  protocol_version: ProtocolVersion
  run_id?: null | string
  step_id: string
  summary?: null | string
  thread_id: string
}

export interface SessionCreateRequest {
  metadata?: JsonObject
  title: string
}

export interface SessionCreateResponse {
  session: SessionRecord
  thread: ThreadRecord
}

export interface ThreadWithSteps {
  steps: StepRecord[]
  thread: ThreadRecord
}

export interface SessionShowResponse {
  session: SessionRecord
  threads: ThreadWithSteps[]
}

export interface ThreadForkRequest {
  metadata?: JsonObject
  parent_thread_id: string
  title?: string
}

export interface ThreadForkResponse {
  parent_thread_id: string
  session_id: string
  thread: ThreadRecord
}

export interface HookSpec {
  command?: string[] | null
  effect?: HookEffect
  enabled?: boolean
  event: HookEventName
  kind: HookKind
  metadata?: JsonObject
  name: string
  protocol_version?: ProtocolVersion
  timeout_ms?: number | null
}

export interface HookEvent {
  agent_id?: null | string
  command?: string[] | null
  duration_ms: number
  error?: JsonObject | null
  finished_at: string
  hook_event: HookEventName
  hook_kind: HookKind
  hook_name: string
  input?: JsonValue
  output?: JsonValue | null
  protocol_version: ProtocolVersion
  run_id?: null | string
  started_at: string
  status: HookInvocationStatus
}

export interface PolicyDecision {
  decision: PolicyDecisionKind
  metadata?: JsonObject
  reason?: null | string
}

export interface ContextPolicy {
  compact_when_over_budget: boolean
  max_input_tokens: number
  preserve_recent_messages: number
  reserve_output_tokens: number
}

export interface ContextBlock {
  block_id: string
  content?: JsonValue
  content_hash: string
  kind: ContextBlockKind
  metadata?: JsonObject
  priority?: number
  source: string
  token_estimate?: number
}

export interface ContextSnapshot {
  blocks?: ContextBlock[]
  compacted?: boolean
  content_hash: string
  created_at: string
  max_input_tokens?: number
  metadata?: JsonObject
  omitted_block_count?: number
  protocol_version: ProtocolVersion
  snapshot_id: string
  token_estimate?: number
}

export interface CompactionRecord {
  after_snapshot_hash: string
  before_snapshot_hash: string
  metadata?: JsonObject
  omitted_block_count: number
  protocol_version: ProtocolVersion
  strategy?: string
  summary?: string
}

export interface AgentRunResponse {
  result: AgentRunResult
  trace: AgentTrace
}

export interface RuntimeMetricsSummary {
  artifact_ref_count: number
  average_run_latency_ms?: null | number
  average_tool_call_latency_ms?: null | number
  failed_run_count: number
  failed_tool_call_count: number
  generated_at: string
  llm_usage_by_provider: {[key: string]: RuntimeLlmProviderMetrics | undefined}
  llm_total_tokens: number
  proposal_applied_count: number
  proposal_approved_count: number
  proposal_count: number
  proposal_created_count: number
  proposal_denied_count: number
  proposals_by_status: {[key: string]: number | undefined}
  protocol_version: ProtocolVersion
  replay_count: number
  run_count: number
  runs_by_agent: {[key: string]: RuntimeAgentMetrics | undefined}
  runs_by_status: {[key: string]: number | undefined}
  runtime_version: string
  skipped_run_count: number
  store_root: string
  successful_run_count: number
  timeout_count: number
  tool_calls_by_tool: {[key: string]: RuntimeToolMetrics | undefined}
  tool_call_count: number
  total_run_latency_ms: number
  total_tool_call_latency_ms: number
}

export interface RuntimeAgentMetrics {
  average_run_latency_ms?: null | number
  failed_run_count: number
  run_count: number
  runs_by_status: {[key: string]: number | undefined}
  successful_run_count: number
  total_run_latency_ms: number
}

export interface RuntimeToolMetrics {
  average_tool_call_latency_ms?: null | number
  failed_tool_call_count: number
  tool_call_count: number
  total_tool_call_latency_ms: number
}

export interface RuntimeLlmProviderMetrics {
  average_latency_ms?: null | number
  cost_micros_by_currency: {[key: string]: number | undefined}
  input_tokens: number
  output_tokens: number
  request_count: number
  total_latency_ms: number
  total_tokens: number
}

export interface LlmMessage {
  content: JsonValue
  metadata?: JsonObject
  name?: null | string
  role: LlmRole
}

export type LlmResponseFormat =
  | {type: 'json_object'}
  | {name: string; schema: JsonObject; strict?: boolean | null; type: 'json_schema'}

export interface LlmRequest {
  max_output_tokens?: null | number
  messages: LlmMessage[]
  metadata?: JsonObject
  model: string
  protocol_version: ProtocolVersion
  provider: string
  response_format?: LlmResponseFormat | null
  temperature?: null | number
  tools?: ToolSpec[]
}

export interface LlmUsage {
  input_tokens: number
  output_tokens: number
  total_tokens: number
}

export interface LlmResponse {
  content: string
  finish_reason: LlmFinishReason
  metadata?: JsonObject
  model: string
  object?: JsonObject | null
  protocol_version: ProtocolVersion
  provider: string
  usage?: LlmUsage | null
}

export interface ChatTurnRequest {
  agent_id?: null | string
  context_policy?: ContextPolicy
  max_output_tokens?: null | number
  max_tool_rounds?: number
  messages: LlmMessage[]
  metadata?: JsonObject
  mode?: null | string
  model: string
  protocol_version?: ProtocolVersion
  provider: string
  session_id?: null | string
  surface?: null | string
  temperature?: null | number
  thread_id?: null | string
  tool_execution?: ChatToolExecution
  tools?: ToolSpec[]
  turn_id?: null | string
}

export interface ChatToolCall {
  id: string
  input?: JsonValue
  name: string
}

export interface ChatToolResult {
  is_error: boolean
  output: JsonValue
  tool_call_id: string
  tool_name: string
}

export interface ChatTurnState {
  agent_id?: null | string
  compaction?: CompactionRecord | null
  context_policy?: ContextPolicy
  context_snapshot?: ContextSnapshot | null
  max_output_tokens?: null | number
  max_tool_rounds: number
  messages: LlmMessage[]
  metadata?: JsonObject
  mode?: null | string
  model: string
  pending_tool_calls: ChatToolCall[]
  protocol_version: ProtocolVersion
  provider: string
  round: number
  session_id?: null | string
  surface?: null | string
  temperature?: null | number
  thread_id?: null | string
  tool_execution?: ChatToolExecution
  tools?: ToolSpec[]
  turn_id?: null | string
}

export interface ChatResumeRequest {
  protocol_version?: ProtocolVersion
  state: ChatTurnState
  tool_results: ChatToolResult[]
}

export type ChatTurnEventKind =
  | 'context_snapshot'
  | 'delta'
  | 'done'
  | 'error'
  | 'llm_started'
  | 'round_finished'
  | 'started'
  | 'thinking_delta'
  | 'thinking_signature_delta'
  | 'tool_call_delta'
  | 'tool_call_end'
  | 'tool_call_start'
  | 'tool_result'
  | 'usage'

export interface ChatTurnEvent {
  content?: null | string
  kind: ChatTurnEventKind
  metadata?: JsonObject
  partial_input_json?: null | string
  response?: LlmResponse | null
  round: number
  tool_call_id?: null | string
  tool_input?: JsonValue
  tool_name?: null | string
  tool_output?: JsonValue
  usage?: LlmUsage | null
}

export type ProposalStatus =
  | 'applied'
  | 'apply_failed'
  | 'applying'
  | 'approved'
  | 'created'
  | 'denied'
  | 'expired'
  | 'pending_approval'
  | 'undone'
  | 'undo_failed'
  | 'undoing'

export interface ProposalEnvelope {
  agent_id: string
  approval_decisions?: ApprovalDecision[]
  approval_policy?: 'auto_approve' | 'manual'
  approval_required?: boolean
  created_at: string
  diffs?: ProposalDiff[]
  expires_at?: null | string
  kind: string
  payload: JsonObject
  policy_id?: null | string
  policy_version?: null | string
  proposal_id: string
  protocol_version: ProtocolVersion
  required_approval_level?: ApprovalLevel
  required_approver_count?: number
  risk?: ToolRisk
  run_id: string
  status: ProposalStatus
  summary: string
  warnings?: ProposalWarning[]
}

export interface ProposalDiff {
  after?: JsonValue
  before?: JsonValue
  metadata?: JsonObject
  operation?: ProposalDiffOperation
  path: string
}

export interface ProposalWarning {
  code: string
  message: string
  metadata?: JsonObject
  severity?: ProposalWarningSeverity
}

export interface ApprovalDecision {
  approval_level?: ApprovalLevel
  comment?: null | string
  decided_by?: null | string
  decided_at: string
  decision: 'approve' | 'deny'
  proposal_id: string
  protocol_version: ProtocolVersion
}

export interface ProposalDecisionRequest {
  approval_level?: ApprovalLevel
  comment?: null | string
  decided_by?: null | string
  decision: 'approve' | 'approved' | 'deny' | 'denied'
}

export interface ProposalActionResponse {
  action: 'apply' | 'undo'
  proposal: ProposalEnvelope
  tool: string
  tool_output: JsonValue
}
