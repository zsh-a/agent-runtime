import type {
  ChatToolExecution,
  JsonObject,
  JsonValue,
  ProtocolVersion,
  ToolSpec,
} from './core.js'
import type {
  CompactionRecord,
  ContextPolicy,
  ContextSnapshot,
} from './hooks.js'
import type {LlmMessage, LlmResponse, LlmUsage} from './llm.js'

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
