import {describe, expect, test} from 'bun:test'

import {AgentRuntimeHttpClient, generateObject, type LlmRequest} from '../src/index.js'

describe('generateObject', () => {
  test('builds a JSON Schema LLM request and returns the typed object', async () => {
    let captured: LlmRequest | undefined
    const result = await generateObject<{title: string}>({
      async complete(request) {
        captured = request
        return {
          content: '{"title":"Runtime"}',
          finish_reason: 'stop',
          model: request.model,
          object: {title: 'Runtime'},
          protocol_version: 'agent.v1',
          provider: request.provider,
        }
      },
    }, {
      messages: [{content: 'summarize', role: 'user'}],
      model: 'gpt-test',
      provider: 'openai-compatible',
      schema: {
        additionalProperties: false,
        properties: {title: {type: 'string'}},
        required: ['title'],
        type: 'object',
      },
      schemaName: 'summary',
    })

    expect(result.object.title).toBe('Runtime')
    expect(captured?.response_format).toEqual({
      name: 'summary',
      schema: {
        additionalProperties: false,
        properties: {title: {type: 'string'}},
        required: ['title'],
        type: 'object',
      },
      strict: true,
      type: 'json_schema',
    })
  })
})

describe('AgentRuntimeHttpClient', () => {
  test('wraps runAgent requests with the runtime HTTP shape', async () => {
    const calls: Array<{body?: string; method?: string; url: string}> = []
    const client = new AgentRuntimeHttpClient({
      baseUrl: 'http://runtime.local/',
      fetch: async (url, init) => {
        calls.push({body: init?.body?.toString(), method: init?.method, url: url.toString()})
        return new Response(JSON.stringify({
          result: {
            agent_id: 'demo',
            finished_at: '2026-07-01T00:00:00Z',
            protocol_version: 'agent.v1',
            run_id: 'run_1',
            started_at: '2026-07-01T00:00:00Z',
            status: 'completed',
          },
          trace: {
            agent_id: 'demo',
            agent_version: '1',
            events: [],
            finished_at: '2026-07-01T00:00:00Z',
            protocol_version: 'agent.v1',
            run_id: 'run_1',
            runtime_version: 'test',
            started_at: '2026-07-01T00:00:00Z',
          },
        }), {status: 200})
      },
    })

    const response = await client.runAgent('demo', {
      input: {project_id: 'p1'},
      sessionId: 'session_1',
      threadId: 'thread_1',
    })

    expect(response.result.run_id).toBe('run_1')
    expect(calls).toEqual([{
      body: JSON.stringify({input: {project_id: 'p1'}, session_id: 'session_1', thread_id: 'thread_1'}),
      method: 'POST',
      url: 'http://runtime.local/agents/demo/run',
    }])
  })

  test('streams chat turn SSE frames as typed events', async () => {
    const calls: Array<{body?: string; method?: string; url: string}> = []
    const client = new AgentRuntimeHttpClient({
      baseUrl: 'http://runtime.local/',
      fetch: async (url, init) => {
        calls.push({body: init?.body?.toString(), method: init?.method, url: url.toString()})
        return new Response([
          'event: chat_turn_event\n',
          'data: {"kind":"started","round":0,"metadata":{}}\n\n',
          'event: chat_turn_event\n',
          'data: {"kind":"delta","content":"hello","round":1,"metadata":{}}\n\n',
          'event: chat_turn_event\n',
          'data: {"kind":"done","round":1,"metadata":{"stop_reason":"end_turn"}}\n\n',
        ].join(''), {
          headers: {'content-type': 'text/event-stream'},
          status: 200,
        })
      },
    })

    const events = []
    for await (const event of client.streamChatTurn({
      messages: [{content: 'ping', role: 'user'}],
      model: 'mock-model',
      provider: 'mock',
      turn_id: 'turn_1',
    })) {
      events.push(event)
    }

    expect(events.map((event) => event.kind)).toEqual(['started', 'delta', 'done'])
    expect(events[1]?.content).toBe('hello')
    expect(calls).toEqual([{
      body: JSON.stringify({
        messages: [{content: 'ping', role: 'user'}],
        model: 'mock-model',
        provider: 'mock',
        turn_id: 'turn_1',
        protocol_version: 'agent.v1',
      }),
      method: 'POST',
      url: 'http://runtime.local/chat/turn',
    }])
  })

  test('streams run events with resumable cursor options', async () => {
    const calls: Array<{headers?: HeadersInit; method?: string; url: string}> = []
    const client = new AgentRuntimeHttpClient({
      baseUrl: 'http://runtime.local/',
      fetch: async (url, init) => {
        calls.push({headers: init?.headers, method: init?.method, url: url.toString()})
        return new Response([
          'id: 3\n',
          'event: run_finished\n',
          'data: {"kind":"run_finished","payload":{"status":"completed"}}\n\n',
        ].join(''), {
          headers: {'content-type': 'text/event-stream'},
          status: 200,
        })
      },
    })

    const events = []
    for await (const event of client.streamRunEvents('run 1', {
      after: 2,
      follow: false,
      lastEventId: 2,
    })) {
      events.push(event)
    }

    expect(events).toEqual([{kind: 'run_finished', payload: {status: 'completed'}}])
    expect(calls).toEqual([{
      headers: {
        accept: 'text/event-stream',
        'Last-Event-ID': '2',
      },
      method: 'GET',
      url: 'http://runtime.local/runs/run%201/events?after=2&follow=false',
    }])
  })
})
