import type {JsonObject, ProtocolVersion, StepKind} from './core.js'

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
