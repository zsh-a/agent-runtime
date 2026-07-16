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
