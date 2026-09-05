# ADR 0002: OpenAI-compatible provider boundary

Status: accepted

## Context

Aegis needs a useful chat path before native llama.cpp and model scanning are
ready. Ollama, LM Studio, vLLM and several self-hosted runtimes already expose
a compatible chat API. Keeping this integration behind one adapter lets the
desktop shell remain independent from provider-specific JSON and streaming
details.

## Decision

The first operational provider is an OpenAI-compatible HTTP adapter in
\`crates/providers\`.

- The native process sends \`GET /models\` and streaming
  \`POST /chat/completions\` requests.
- The frontend receives typed Tauri events, never raw provider responses.
- Provider API keys are accepted only as transient command input and are not
  persisted by the frontend.
- SSE frames are size-bounded, parsed incrementally and reject malformed JSON.
- Cancellation is propagated through the active operation token.
- Provider output remains data. It cannot bypass the Permission Broker or
  authorize a tool action.

## Consequences

This slice is immediately usable with local servers and makes remote routing
explicit. It does not replace the supervised llama.cpp worker; native GGUF
loading, model scanning, hardware telemetry and tool execution remain separate
milestones with their own isolation and approval requirements.
