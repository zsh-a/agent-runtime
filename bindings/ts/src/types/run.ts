import type {
  AgentRunStatus,
  ArtifactKind,
  JsonObject,
  JsonValue,
  ProtocolVersion,
  RedactionClassification,
  ReplayMode,
  ToolRisk,
  TriggerKind,
  UserContext,
} from './core.js'

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

export type EmbeddedRunStepStatus =
  | 'effect_requested'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'policy_denied'
  | 'closed_early'
  | 'timed_out'

export type EmbeddedTerminalReason =
  | 'done'
  | 'stream_error'
  | 'user_cancel'
  | 'policy_denied'
  | 'closed_early'

export type EmbeddedEffectKind = 'tool' | 'subagent'

export type EmbeddedHostEffect =
  | {
      effect_id: string
      input: JsonObject
      kind: 'tool'
      metadata: JsonObject
      name: string
      risk: ToolRisk
    }
  | {
      agent_id: string
      effect_id: string
      input: JsonObject
      kind: 'subagent'
      metadata: JsonObject
      run_id?: string | null
      scope?: RunScope | null
      workflow?: RunWorkflow | null
    }

export type EmbeddedPendingHostEffect =
  | {input: JsonObject; kind: 'tool'; name: string}
  | {
      agent_id: string
      input: JsonObject
      kind: 'subagent'
      metadata: JsonObject
      run_id?: string | null
      scope?: RunScope | null
      workflow?: RunWorkflow | null
    }

export interface EmbeddedEffectError {
  code: number
  data?: JsonValue
  message: string
  [key: string]: JsonValue | undefined
}

export type EmbeddedEffectResponse =
  | {error: EmbeddedEffectError; id: string; jsonrpc: '2.0'; result?: never}
  | {error?: never; id: string; jsonrpc: '2.0'; result: JsonValue}

export interface EmbeddedEffectResult {
  effect: EmbeddedHostEffect
  effect_response: EmbeddedEffectResponse
  kind: EmbeddedEffectKind
}

export interface EmbeddedRunContinuation {
  effect_results: EmbeddedEffectResult[]
  effects: EmbeddedPendingHostEffect[]
  llm_response?: JsonValue
  next_step_index: number
}

export interface EmbeddedRunState {
  effect_result_count: number
  remaining_effect_count: number
  status: EmbeddedRunStepStatus
  step_index: number
  terminal_reason: EmbeddedTerminalReason | null
}

export interface EmbeddedStepTraceEvent {
  agent_id: string
  effect_id: null | string
  effect_kind: EmbeddedEffectKind | null
  kind: 'agent_runtime_step'
  run_id: string
  run_state: EmbeddedRunState
  status: EmbeddedRunStepStatus
  step_index: number
  subagent_id: null | string
  tool_name: null | string
}

export interface EmbeddedRunStep {
  agent_id: string
  agent_version: string
  continuation?: EmbeddedRunContinuation | null
  effect?: EmbeddedHostEffect | null
  effect_response?: EmbeddedEffectResponse | null
  effect_result?: JsonValue
  effect_results?: EmbeddedEffectResult[]
  error?: JsonValue
  output?: JsonValue
  proposal?: JsonValue
  protocol_version: ProtocolVersion
  run_id: string
  run_state: EmbeddedRunState
  status: EmbeddedRunStepStatus
  step_index: number
  trace_event: EmbeddedStepTraceEvent
}

export interface EmbeddedRunLimits {
  max_effect_steps: number
  max_subagent_depth: number
}

export interface EmbeddedRunProgress {
  dispatched_effect_count: number
  effect_budget_exhausted: boolean
  subagent_depth: number
  subagent_depth_exceeded: boolean
}

export interface EmbeddedRunSnapshot {
  limits: EmbeddedRunLimits
  progress: EmbeddedRunProgress
  protocol_version: ProtocolVersion
  snapshot_version: 1
  step: EmbeddedRunStep
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
  version: number
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
