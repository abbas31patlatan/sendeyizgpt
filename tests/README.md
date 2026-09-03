# Test strategy

The Rust crates contain fast unit tests for protocol framing, IPC
authentication, broker invariants, agent budgets, resource estimates and
database migrations. CI runs them on Windows because the release target is
Windows/MSVC.

The next integration test layers are deliberately named here before their
effectful workers exist:

- `permission-no-side-effect`: a model-shaped proposal cannot execute shell,
  write, delete, process, network or input actions without a permit.
- `path-boundary`: traversal, symlink, junction/reparse-point and case-folding
  cases stay inside the explicit workspace root.
- `ipc-auth`: wrong version, wrong session key, replayed request and malformed
  payloads are rejected.
- `worker-restart`: inference worker exit preserves the UI/core state and
  returns a structured, recoverable error.
- `gguf-fuzz`: bounded metadata parsing rejects oversized, malformed and
  adversarial GGUF input without taking down the desktop process.
- `untrusted-content`: HTML/document/MCP output cannot be serialized as a
  system/developer instruction or mutate broker policy.

No test should rely on a real model, real credential or a user's private path.

