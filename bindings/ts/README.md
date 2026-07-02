# Agent Runtime TypeScript Bindings

Small, dependency-light TypeScript bindings for the `agent.v1` JSON contracts.

The package intentionally keeps runtime concerns separate:

- `types.ts` mirrors stable wire contracts.
- `http-client.ts` talks to the Agent Runtime HTTP server.
- `generate-object.ts` builds structured LLM requests over any `complete()`
  transport.

`generateObject<T>()` accepts JSON Schema, not Zod. Host apps can convert Zod or
other schema systems before calling it.
