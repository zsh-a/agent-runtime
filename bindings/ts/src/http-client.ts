import type {
  AgentRunRecord,
  AgentRunResponse,
  AgentTrace,
  ApprovalDecision,
  CancelRunResponse,
  ChatResumeRequest,
  ChatTurnEvent,
  ChatTurnRequest,
  JsonObject,
  ProposalActionResponse,
  ProposalDecisionRequest,
  ProposalEnvelope,
  ReplayExecutionResponse,
  RuntimeMetricsSummary,
  SessionCreateRequest,
  SessionCreateResponse,
  SessionRecord,
  SessionShowResponse,
  ThreadForkRequest,
  ThreadForkResponse,
  ToolSpec,
  TriggerEnvelope,
  TriggerKind,
  UserContext,
  RunScope,
  RunWorkflow,
  WorkflowRunRequest,
  WorkflowRunResult,
} from './types.js'

export interface AgentRuntimeHttpClientOptions {
  baseUrl: string
  fetch?: typeof fetch
}

export interface RunAgentParams {
  input?: JsonObject
  metadata?: JsonObject
  runId?: string
  sessionId?: string
  threadId?: string
  trigger?: TriggerKind
  triggerEnvelope?: TriggerEnvelope | null
  scope?: RunScope | null
  user?: UserContext | null
  workflow?: RunWorkflow | null
}

export type RunWorkflowParams = Omit<WorkflowRunRequest, 'protocol_version'> & {
  protocol_version?: WorkflowRunRequest['protocol_version']
}

export class AgentRuntimeHttpError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly body: unknown,
  ) {
    super(message)
    this.name = 'AgentRuntimeHttpError'
  }
}

export class AgentRuntimeHttpClient {
  private readonly baseUrl: string
  private readonly fetchImpl: typeof fetch

  constructor(options: AgentRuntimeHttpClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '')
    this.fetchImpl = options.fetch ?? fetch
  }

  healthz(): Promise<{status: 'ok'}> {
    return this.request('GET', '/healthz')
  }

  metricsSummary(): Promise<RuntimeMetricsSummary> {
    return this.request('GET', '/metrics/summary')
  }

  listTools(): Promise<ToolSpec[]> {
    return this.request('GET', '/tools')
  }

  async *streamChatTurn(request: ChatTurnRequest): AsyncGenerator<ChatTurnEvent> {
    const response = await this.fetchImpl(`${this.baseUrl}/chat/turn`, {
      body: JSON.stringify({
        ...request,
        protocol_version: request.protocol_version ?? 'agent.v1',
      }),
      headers: {
        accept: 'text/event-stream',
        'content-type': 'application/json',
      },
      method: 'POST',
    })

    if (!response.ok) {
      const payload = await readResponseBody(response)
      throw new AgentRuntimeHttpError(readErrorMessage(payload, response.statusText), response.status, payload)
    }
    if (response.body === null) {
      throw new AgentRuntimeHttpError('Agent Runtime chat stream response had no body', response.status, undefined)
    }

    for await (const data of readServerSentEventData(response.body)) {
      yield JSON.parse(data) as ChatTurnEvent
    }
  }

  async *streamChatResume(request: ChatResumeRequest): AsyncGenerator<ChatTurnEvent> {
    const response = await this.fetchImpl(`${this.baseUrl}/chat/resume`, {
      body: JSON.stringify({
        ...request,
        protocol_version: request.protocol_version ?? 'agent.v1',
      }),
      headers: {
        accept: 'text/event-stream',
        'content-type': 'application/json',
      },
      method: 'POST',
    })

    if (!response.ok) {
      const payload = await readResponseBody(response)
      throw new AgentRuntimeHttpError(readErrorMessage(payload, response.statusText), response.status, payload)
    }
    if (response.body === null) {
      throw new AgentRuntimeHttpError('Agent Runtime chat resume stream response had no body', response.status, undefined)
    }

    for await (const data of readServerSentEventData(response.body)) {
      yield JSON.parse(data) as ChatTurnEvent
    }
  }

  runAgent(agentId: string, params: RunAgentParams = {}): Promise<AgentRunResponse> {
    return this.request('POST', `/agents/${encodeURIComponent(agentId)}/run`, {
      input: params.input ?? {},
      ...(params.metadata === undefined ? {} : {metadata: params.metadata}),
      ...(params.runId === undefined ? {} : {run_id: params.runId}),
      ...(params.sessionId === undefined ? {} : {session_id: params.sessionId}),
      ...(params.threadId === undefined ? {} : {thread_id: params.threadId}),
      ...(params.trigger === undefined ? {} : {trigger: params.trigger}),
      ...(params.triggerEnvelope === undefined ? {} : {trigger_envelope: params.triggerEnvelope}),
      ...(params.scope === undefined ? {} : {scope: params.scope}),
      ...(params.user === undefined ? {} : {user: params.user}),
      ...(params.workflow === undefined ? {} : {workflow: params.workflow}),
    })
  }

  runWorkflow(request: RunWorkflowParams): Promise<WorkflowRunResult> {
    return this.request('POST', '/workflows/run', {
      ...request,
      protocol_version: request.protocol_version ?? 'agent.v1',
    })
  }

  listRuns(params: {agentId?: string; limit?: number} = {}): Promise<AgentRunRecord[]> {
    const query = new URLSearchParams()
    if (params.agentId !== undefined) {
      query.set('agent_id', params.agentId)
    }
    if (params.limit !== undefined) {
      query.set('limit', String(params.limit))
    }
    const suffix = query.size === 0 ? '' : `?${query.toString()}`

    return this.request('GET', `/runs${suffix}`)
  }

  getRun(runId: string): Promise<AgentRunRecord> {
    return this.request('GET', `/runs/${encodeURIComponent(runId)}`)
  }

  getRunTrace(runId: string): Promise<AgentTrace> {
    return this.request('GET', `/runs/${encodeURIComponent(runId)}/trace`)
  }

  async *streamRunEvents(runId: string): AsyncGenerator<AgentTrace['events'][number]> {
    const response = await this.fetchImpl(`${this.baseUrl}/runs/${encodeURIComponent(runId)}/events`, {
      headers: {
        accept: 'text/event-stream',
      },
      method: 'GET',
    })

    if (!response.ok) {
      const payload = await readResponseBody(response)
      throw new AgentRuntimeHttpError(readErrorMessage(payload, response.statusText), response.status, payload)
    }
    if (response.body === null) {
      throw new AgentRuntimeHttpError('Agent Runtime run event stream response had no body', response.status, undefined)
    }

    for await (const data of readServerSentEventData(response.body)) {
      yield JSON.parse(data) as AgentTrace['events'][number]
    }
  }

  cancelRun(runId: string): Promise<CancelRunResponse> {
    return this.request('POST', `/runs/${encodeURIComponent(runId)}/cancel`, {})
  }

  replayRun(runId: string): Promise<ReplayExecutionResponse> {
    return this.request('POST', `/runs/${encodeURIComponent(runId)}/replay`)
  }

  callTool<TOutput = unknown>(toolName: string, input: JsonObject = {}): Promise<{output: TOutput; tool: string}> {
    return this.request('POST', `/tools/${encodeURIComponent(toolName)}/call`, {input})
  }

  listSessions(): Promise<SessionRecord[]> {
    return this.request('GET', '/sessions')
  }

  createSession(input: SessionCreateRequest): Promise<SessionCreateResponse> {
    return this.request('POST', '/sessions', input)
  }

  showSession(sessionId: string): Promise<SessionShowResponse> {
    return this.request('GET', `/sessions/${encodeURIComponent(sessionId)}`)
  }

  forkSessionThread(sessionId: string, input: ThreadForkRequest): Promise<ThreadForkResponse> {
    return this.request('POST', `/sessions/${encodeURIComponent(sessionId)}/fork`, input)
  }

  listProposals(runId?: string): Promise<ProposalEnvelope[]> {
    const suffix = runId === undefined ? '' : `?${new URLSearchParams({run_id: runId}).toString()}`

    return this.request('GET', `/proposals${suffix}`)
  }

  createProposal(
    input: Pick<ProposalEnvelope, 'agent_id' | 'diffs' | 'kind' | 'payload' | 'run_id' | 'summary' | 'warnings'>,
  ): Promise<ProposalEnvelope> {
    return this.request('POST', '/proposals', input)
  }

  decideProposal(
    proposalId: string,
    decision: 'approve' | 'deny',
    commentOrOptions?: string | Omit<ProposalDecisionRequest, 'decision'>,
  ): Promise<{decision: ApprovalDecision; proposal: ProposalEnvelope}> {
    const body =
      typeof commentOrOptions === 'string' || commentOrOptions === undefined
        ? {comment: commentOrOptions, decision}
        : {...commentOrOptions, decision}

    return this.request('POST', `/proposals/${encodeURIComponent(proposalId)}/decision`, body)
  }

  applyProposal(proposalId: string): Promise<ProposalActionResponse> {
    return this.request('POST', `/proposals/${encodeURIComponent(proposalId)}/apply`)
  }

  undoProposal(proposalId: string): Promise<ProposalActionResponse> {
    return this.request('POST', `/proposals/${encodeURIComponent(proposalId)}/undo`)
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const init: RequestInit = {method}
    if (body !== undefined) {
      init.body = JSON.stringify(body)
      init.headers = {'content-type': 'application/json'}
    }

    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      ...init,
    })
    const payload = await readResponseBody(response)

    if (!response.ok) {
      throw new AgentRuntimeHttpError(readErrorMessage(payload, response.statusText), response.status, payload)
    }

    return payload as T
  }
}

async function readResponseBody(response: Response): Promise<unknown> {
  const text = await response.text()
  if (text.trim() === '') {
    return undefined
  }

  try {
    return JSON.parse(text) as unknown
  } catch {
    return text
  }
}

function readErrorMessage(payload: unknown, fallback: string): string {
  if (isRecord(payload) && typeof payload.message === 'string') {
    return payload.message
  }

  return fallback || 'Agent Runtime request failed'
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

async function* readServerSentEventData(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  try {
    while (true) {
      const {done, value} = await reader.read()
      if (done) {
        break
      }
      buffer += decoder.decode(value, {stream: true})
      const drained = drainServerSentEventFrames(buffer)
      buffer = drained.rest
      for (const data of drained.data) {
        yield data
      }
    }
    buffer += decoder.decode()
    const drained = drainServerSentEventFrames(buffer)
    for (const data of drained.data) {
      yield data
    }
    const tail = drained.rest.trim()
    if (tail.length > 0) {
      const data = readServerSentEventFrameData(tail)
      if (data !== undefined) {
        yield data
      }
    }
  } finally {
    reader.releaseLock()
  }
}

function drainServerSentEventFrames(buffer: string): {data: string[]; rest: string} {
  const data: string[] = []
  let rest = buffer

  while (true) {
    const boundary = nextServerSentEventBoundary(rest)
    if (boundary === undefined) {
      return {data, rest}
    }

    const frame = rest.slice(0, boundary.index)
    rest = rest.slice(boundary.index + boundary.length)
    const frameData = readServerSentEventFrameData(frame)
    if (frameData !== undefined) {
      data.push(frameData)
    }
  }
}

function nextServerSentEventBoundary(buffer: string): {index: number; length: number} | undefined {
  const lf = buffer.indexOf('\n\n')
  const crlf = buffer.indexOf('\r\n\r\n')
  if (lf === -1 && crlf === -1) {
    return undefined
  }
  if (lf === -1) {
    return {index: crlf, length: 4}
  }
  if (crlf === -1 || lf < crlf) {
    return {index: lf, length: 2}
  }
  return {index: crlf, length: 4}
}

function readServerSentEventFrameData(frame: string): string | undefined {
  const lines = frame.split(/\r?\n/)
  const data = lines
    .filter((line) => line.startsWith('data:'))
    .map((line) => line.slice(5).replace(/^ /, ''))

  return data.length === 0 ? undefined : data.join('\n')
}
