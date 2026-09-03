# Plugin API foundation

The plugin API is a compatibility boundary, not an import convention.

## Manifest

Every package must provide a signed, canonical manifest resembling:

```json
{
  "id": "example.tool.files",
  "name": "Example Files Tool",
  "version": "1.0.0",
  "api_major": 1,
  "api_minor": 0,
  "kind": "tool",
  "entrypoint": "plugin.wasm",
  "capabilities": ["filesystem.read"],
  "risk_level": "read_only",
  "permissions_explanation": "Reads files only inside a user-selected workspace",
  "signature": {
    "algorithm": "ed25519",
    "key_id": "publisher-key-id",
    "value": "base64-signature"
  }
}
```

The actual signed bytes exclude the signature field and use canonical JSON.
Manifests are size-limited and parsed before the plugin process starts.

## Compatibility

- `api_major` must match exactly.
- `api_minor` may be lower than or equal to the host-supported minor version
  when the plugin declares feature negotiation.
- Unknown required features cause a controlled rejection.
- A rejected plugin is visible in the Plugin Manager with the reason; it is not
  silently loaded with partial permissions.

## Host contract

The host exposes typed operations for manifest discovery, health, cancellation,
structured events, preview, broker request and redacted result reporting. A
plugin never receives a raw desktop-core pointer, SQLite connection, OS secret,
unrestricted socket or arbitrary path.

## Isolation policy

1. WASM component for deterministic, pure extensions.
2. Separate process for native code, device access, network or MCP.
3. In-process native loading is developer-only and disabled in release builds
   until a separately reviewed trust model exists.

## Versioning rule

New optional fields are additive. Removing or changing the meaning of an
existing field requires a major API. A plugin can advertise capability support
but the Permission Broker still decides per invocation.

