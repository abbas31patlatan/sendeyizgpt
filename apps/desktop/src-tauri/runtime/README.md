# Native llama.cpp runtime

The Windows packaging workflow builds llama-server.exe from the official
[ggml-org/llama.cpp](https://github.com/ggml-org/llama.cpp) source at the pinned
commit recorded in LLAMA_CPP_BUILD.txt. The upstream `LICENSE` is copied as
LLAMA_CPP_LICENSE.txt. Generated binaries, license staging and build metadata
are intentionally ignored by Git and are copied into Tauri resources during
release packaging.

Aegis starts this executable with a validated local GGUF path, loopback-only
binding, bounded load-profile flags and offline mode. It waits for the native
server health endpoint before routing chat traffic to its OpenAI-compatible API.

Developers may leave this directory empty and provide a compatible llama-server
on PATH or enter an explicit executable path in the model runtime panel.
