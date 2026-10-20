# field-eval

Offline tactical LLM and survival telemetry utility. One Rust binary, one
terminal UI, zero sockets.

- **Inference:** `rig-llama-cpp` → `llama-cpp-2` → vendored llama.cpp, CPU SIMD by default.
- **Agent layer:** `rig-agent` on `rig-core` (streaming completions, prompt control).
- **Knowledge:** markdown manuals compiled into the binary with `include_str!` and
  indexed at startup by a small inverted index — no files, no database, no network.
- **UI:** `ratatui` + `crossterm`, three night-vision palettes (red / green / amber).

## Build

```sh
./scripts/build.sh build --release
```

`scripts/build.sh` exists because this host is a PRoot Debian rootfs on Android
with two toolchains on `PATH`. Four traps it neutralises:

| Trap | Symptom | Fix in the script |
|---|---|---|
| Termux clang shadows Debian gcc | `ld.lld: unable to find library -lgcc_s` | `PATH` puts `/usr/bin` first; linker pinned to `/usr/bin/cc` in `.cargo/config.toml` |
| Termux CMake 4.x | `CMakeDetermineSystem` reads `$PREFIX/include/android/api-level.h` | Debian cmake 3.25 (`apt install cmake`); `PREFIX` intentionally unset |
| bindgen picks bionic headers | `Unversioned target triples are not supported!` | `LIBCLANG_PATH=/usr/lib/llvm-14/lib` plus explicit glibc include args |
| Termux pkg-config leaks its prefix | Debian g++ compiles bionic `time.h`; cpp-httplib fails on `nanosleep` | Debian `pkg-config` + `libssl-dev` installed |

Host packages required: `build-essential cmake pkg-config libssl-dev libclang-dev`.

Memory: llama.cpp is compiled with two jobs (`CMAKE_BUILD_PARALLEL_LEVEL=2`).
Raise it only if the machine has headroom.

## Run

```sh
./target/release/field-eval --model /path/to/model.gguf
```

```sh
./target/release/field-eval --model /path/to/model.gguf --ask "brake pedal is spongy"
```

Add `--ctx N` (or `FIELD_EVAL_CTX`) to trade context for memory. On this phone,
1024 tokens with a 3B Q4 model leaves enough headroom; 4096 competes with swap.

Without `--model` (or with `FIELD_EVAL_MODEL` unset) the binary starts in
retrieval mode: it returns the matching manual sections verbatim. That path is
useful for verifying the corpus on a machine with no GGUF file.

Keys: `enter` ask · `tab` cycle palette · `pgup/pgdn` scroll · `ctrl-l` clear ·
`ctrl-c` quit · `esc` cancel generation.

Side-lining: `--selftest` exercises six retrieval cases and exits non-zero on
regression.

## Layout

```
src/main.rs    CLI, model loading, generation, event loop
src/tui.rs     ratatui rendering, palettes, key handling, wrapping
src/corpus.rs  include_str! corpus, section splitter, inverted index, search
assets/*.md    manually authored reference material, split on "## " headings
scripts/       host-specific build wrapper
```

## Corpus format

Every `## Heading` starts a retrievable section. Section headings are scored
with a 2× boost, so name them after the symptom a user would say aloud
("Engine cranks but will not start", not "Starting system"). Lowercase
plurals are stemmed crudely, and stopwords are dropped.

## Known limits

- **CPU only on this host.** `--features vulkan` exists but cannot work here:
  the PRoot rootfs has no Vulkan loader or ICD, and `/dev/dri` is permission
  denied. The `vulkan` feature is forwarded to `rig-llama-cpp` for hosts where
  it does work.
- Context defaults to 4096 tokens; retrieval injects at most three sections,
  dropping any scoring under a third of the best match.
- The corpus is authored, not scraped. Treat it as a reference, not medical or
  mechanical authority.

## Measured on this device

Qwen2.5-3B-Instruct Q4_K_M (1.8 GB), 4 cores, `--ctx 1024`: **58 s** for a
two-sentence cited answer, ~2 GB RSS. Prefill dominates; decode is a few tokens
per second. Two concurrent runs exhaust RAM — the second is killed by the kernel
with no message, so treat one resident model as the budget.

## Two traps specific to this crate

- `Client::drop` shuts the inference worker down **synchronously**. Taking
  `client.completion_model(..)` and dropping the client compiles cleanly and
  then fails every request with `Inference thread shut down`. Keep the `Client`
  alive alongside the `Model` — see `Loaded` in `src/main.rs`.
- Never put a placeholder citation in the system prompt. `[doc.md § Section]`
  was copied verbatim into answers as `doc.md § 2.1.1`. Name the real files and
  headings.
