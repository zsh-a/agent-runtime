import type {
  JsonObject,
  JsonValue,
  LlmFinishReason,
  LlmRole,
  ProtocolVersion,
  ToolSpec,
} from './core.js'

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
