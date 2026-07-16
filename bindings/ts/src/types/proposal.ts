import type {
  ApprovalLevel,
  JsonObject,
  JsonValue,
  ProposalDiffOperation,
  ProposalWarningSeverity,
  ProtocolVersion,
  ToolRisk,
} from './core.js'

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
  version: number
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
