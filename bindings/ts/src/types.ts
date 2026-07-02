export type ProtocolVersion = 'agent.v1'

export type JsonPrimitive = boolean | null | number | string
export type JsonValue = JsonPrimitive | JsonValue[] | {[key: string]: JsonValue | undefined}
export type JsonObject = {[key: string]: JsonValue | undefined}

export type TriggerKind = 'manual' | 'replay' | 'scheduled'
export type AgentRunStatus = 'abandoned' | 'cancelled' | 'completed' | 'failed' | 'running' | 'skipped' | 'timed_out'
export type ToolRisk = 'high' | 'low' | 'medium' | 'read_only'
export type LlmRole = 'assistant' | 'system' | 'tool' | 'user'
export type LlmFinishReason = 'content_filter' | 'error' | 'length' | 'stop' | 'tool_call'

export interface UserContext {
  metadata?: JsonObject
  user_id: string
}

export type ScheduleSpec =
  | {type: 'manual'}
  | {every_seconds: number; jitter_seconds?: number | null; preferred_hour_local?: number | null; type: 'interval'}

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
  trigger?: TriggerKind
  user?: UserContext | null
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
  scope: JsonObject
  started_at: string
  status: AgentRunStatus
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
}

export interface AgentErrorRecord {
  code: string
  details?: JsonObject
  kind: string
  message: string
  retryable: boolean
}

export interface TraceEvent {
  kind: string
  occurred_at: string
  payload?: JsonObject
}

export interface AgentTrace {
  agent_id: string
  agent_version: string
  events: TraceEvent[]
  finished_at: string
  input?: JsonObject
  output?: JsonObject
  protocol_version: ProtocolVersion
  run_id: string
  runtime_version: string
  started_at: string
}

export interface AgentRunResponse {
  result: AgentRunResult
  trace: AgentTrace
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
  tools?: ToolSpec[]
  turn_id?: null | string
}

export type ChatTurnEventKind =
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
  created_at: string
  expires_at?: null | string
  kind: string
  payload: JsonObject
  proposal_id: string
  protocol_version: ProtocolVersion
  run_id: string
  status: ProposalStatus
  summary: string
}

export interface ApprovalDecision {
  comment?: null | string
  decided_at: string
  decision: 'approve' | 'deny'
  proposal_id: string
  protocol_version: ProtocolVersion
}
