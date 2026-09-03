# Third-party license inventory

This file is the source-of-truth index for dependency and runtime license review.
It is intentionally kept separate from the application license decision.

## Initial runtime/dependency inventory

| Component | Role | License / review status |
| --- | --- | --- |
| Tauri 2 | Native desktop shell | Review the exact version's bundled notices at release time |
| React / React DOM | Frontend UI | MIT |
| Vite | Frontend build | MIT |
| Tokio | Async runtime | MIT |
| Serde / serde_json | Typed serialization | MIT / Apache-2.0 |
| jsonschema | Tool/plugin input validation | MIT; remote reference features disabled in the core |
| rusqlite + SQLite | Persistence | rusqlite: MIT; SQLite: public domain |
| llama.cpp worker | Local inference runtime | MIT; preserve upstream notices |
| GGUF model files | User-provided model assets | Model-specific license; never infer from application license |

## Release requirements

1. Generate a complete dependency tree for Rust and npm at release time.
2. Store exact versions and notices in the release artifact.
3. Ship upstream llama.cpp and model licenses where applicable.
4. Record plugin and MCP server licenses independently from the core product.
5. Do not redistribute a model unless its model license explicitly permits it.

The application license remains an explicit product decision and is not selected by
the bootstrap milestone.
