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

## Trust zones

| Zone | Examples | Allowed authority |
| --- | --- | --- |
| T0: trusted core | Permission Broker, migration engine, worker supervisor | Policy, persistence coordination, permit issuance |
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

## Deliberate limitations in Milestone 0

The current foundation does not expose host filesystem, shell, browser,
microphone, camera, screen or keyboard/mouse executors. This is intentional:
the security contract is present before effectful features are added.

