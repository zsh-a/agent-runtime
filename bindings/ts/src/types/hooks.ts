import type {
  ContextBlockKind,
  HookEffect,
  HookEventName,
  HookInvocationStatus,
  HookKind,
  JsonObject,
  JsonValue,
  PolicyDecisionKind,
  ProtocolVersion,
} from './core.js'

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
