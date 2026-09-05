# Security model

Security is a runtime invariant, not a prompt-writing convention.

## Hard rules

1. Model output is data. It can propose a tool action but cannot execute one.
2. Permission policy lives in the native core and is not model-editable.
3. Every side effect requires a broker-issued permit bound to the exact action.
4. High-risk operations are non-persistent in strict mode and default policy is
   safe for a new installation.
5. External content is untrusted and cannot change trusted instructions or policy.
6. Microphone, camera, screen capture, network side effects and input injection
   are OFF until the user starts a visible, scoped session.
7. Secrets are stored only through the OS secret adapter and are redacted from
   logs, prompts and diagnostic bundles.
8. Provider API keys are session-only in the current chat slice and are never
   written to SQLite, localStorage or the workspace registry.
9. Public web retrieval is HTTPS-only, rejects loopback/private/link-local hosts,
   resolves and pins approved public DNS answers, follows only revalidated
   redirects and returns bounded untrusted source material.
10. Tool schemas and arguments are bounded at the native boundary. Tool failures
    are returned as data for correction, never as executable instructions.

## Local model catalog boundary

The local catalog is deliberately metadata-only and user initiated:

- Only a registered, canonicalized directory is scanned; symlinks are not followed.
- Traversal, file count, metadata size, string size, array size and nesting are bounded.
- GGUF headers and metadata are parsed during cataloging; tensor data is not loaded or executed by the scanner.
- A corrupt file is returned as a non-fatal issue while valid models remain indexable.
- Model metadata is persisted as bounded SQLite data; it is not a permission grant or
  an instruction source for a model, agent or tool.
- Load estimates and profiles are advisory before launch. The native runtime's
  explicit **Ready** state is the evidence that `llama-server` accepted the GGUF;
  it still does not claim that every estimate or device metric is sustainable.

## Native runtime boundary

Native tensor loading is a separate, supervised process boundary:

- A start request must name a cataloged model. The shell re-inspects the GGUF and
  requires the ID, exact file size and metadata hash to match the SQLite record.
- The executable is either the packaged pinned runtime, a user-selected regular
  file or a bare command resolved through PATH. No shell string is constructed.
- The model path and profile settings are passed as an argument vector; stdin and
  stdout/stderr are detached, and Windows uses a hidden child-process window.
- `llama-server` binds only to `127.0.0.1` on a random reserved port, starts in
  offline mode and must pass `/health` before the UI marks tensors **Ready**.
- Tauri owns the child lifecycle, generation guard, failure state and kill/wait
  cleanup on unload, startup cancellation and application shutdown.
- Native chat uses the loopback `/v1` endpoint without a provider API key; the
  existing bounded streaming/reasoning/cancel client remains in force.
- The bundled release is CPU-native. GPU acceleration requires a compatible
  external llama.cpp build selected explicitly by path or PATH.

## Agent tool boundary

The current agent loop exposes only read-only built-ins and a controlled public-web
retrieval path:

- Calculator, UTC clock, JSON formatting and text statistics do not touch the host.
- Web search uses a fixed HTTPS search endpoint whose DNS answers are checked and
  pinned for the request. Page retrieval accepts only HTTPS public hosts, resolves
  and checks DNS answers for private/local ranges, disables automatic redirects,
  revalidates each redirect target, limits response bytes and strips active HTML
  blocks before the text enters the model context.
- Master-to-worker delegation accepts only a bounded task/context payload. The
  worker receives a separate request with tools disabled; timeout, transport and
  provider failures are handled by a master fallback.
- Tool calls are executed concurrently only within the current request, each
  outcome is emitted to the UI, and malformed arguments can be corrected at most
  three times per tool before the request fails closed.
- Web pages, search snippets, provider responses and tool output remain data. They
  are never appended as system/developer instructions.

## Trust zones

| Zone | Examples | Allowed authority |
| --- | --- | --- |
| T0: trusted core | Permission Broker, migration engine, workspace registry | Policy, persistence coordination, path-scope registration, permit issuance |
| T1: controlled worker | Agent, inference, tool, audio, vision | Only the capability and IPC contract granted to it |
| T2: untrusted content | Web pages, MCP results, documents, emails, provider responses | Data only; no policy or instruction authority |
| T3: user/device | User files, OS devices, external network | Access only through an explicit broker decision |

## Approval contract

An approval is two-phase:

1. Preview: the tool produces a typed description of what it wants to do.
2. Commit: the user approves the exact preview; the broker returns a one-time
   permit; the executor consumes it immediately before the effect.

Changing a path, command, URL, arguments, environment implication or diff
changes the request digest and invalidates the permit.

## Reporting vulnerabilities

Do not include real API keys, personal files or private model assets in an issue.
The first report should include the smallest reproducible input, affected
worker/core boundary, platform and whether a permit was required.

## Deliberate limitations after M3b

The current product does not expose host filesystem, shell, browser, microphone,
camera, screen or keyboard/mouse executors. Native GGUF tensor loading and the
read-only web/tool loop are real and supervised, but GPU capability detection,
detailed device telemetry and effectful tool workers remain separate milestones.
Workspace registration is still only read-only path metadata validation, and
neither it nor model loading is a file access grant.
