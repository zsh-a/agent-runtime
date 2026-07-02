import type {JsonObject, LlmMessage, LlmRequest, LlmResponse, ToolSpec} from './types.js'

export interface LlmCompleteTransport {
  complete(request: LlmRequest): Promise<LlmResponse>
}

export interface GenerateObjectRequest<T> {
  maxOutputTokens?: number
  messages: LlmMessage[]
  metadata?: JsonObject
  model: string
  parse?: (value: unknown) => T
  provider: string
  schema: JsonObject
  schemaName: string
  strict?: boolean
  temperature?: number
  tools?: ToolSpec[]
}

export interface GenerateObjectResult<T> {
  object: T
  request: LlmRequest
  response: LlmResponse
}

export async function generateObject<T>(
  transport: LlmCompleteTransport,
  input: GenerateObjectRequest<T>,
): Promise<GenerateObjectResult<T>> {
  const request = createGenerateObjectRequest(input)
  const response = await transport.complete(request)
  const rawObject = response.object ?? parseObjectContent(response.content)
  const object = input.parse === undefined ? rawObject as T : input.parse(rawObject)

  return {object, request, response}
}

export function createGenerateObjectRequest<T>(input: GenerateObjectRequest<T>): LlmRequest {
  return {
    messages: input.messages,
    metadata: input.metadata ?? {},
    model: input.model,
    protocol_version: 'agent.v1',
    provider: input.provider,
    ...(input.maxOutputTokens === undefined ? {} : {max_output_tokens: input.maxOutputTokens}),
    response_format: {
      name: input.schemaName,
      schema: input.schema,
      strict: input.strict ?? true,
      type: 'json_schema',
    },
    ...(input.temperature === undefined ? {} : {temperature: input.temperature}),
    tools: input.tools ?? [],
  }
}

function parseObjectContent(content: string): JsonObject {
  let value: unknown

  try {
    value = JSON.parse(content)
  } catch (error) {
    throw new Error(`LLM response did not contain JSON object content: ${formatError(error)}`)
  }

  if (!isJsonObject(value)) {
    throw new Error('LLM response JSON content must be an object.')
  }

  return value
}

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
