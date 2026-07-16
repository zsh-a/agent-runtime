import type {ProtocolVersion} from './core.js'
import type {AgentRunResult, AgentTrace} from './run.js'

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
