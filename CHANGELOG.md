# Changelog

## 0.1.0-alpha.1 — 2026-10-20

First alpha. Functional end to end on aarch64 / PRoot Debian.

### Added
- Single-binary offline inference over GGUF via `rig-llama-cpp` (CPU SIMD).
- Embedded corpus: first aid, survival, vehicle troubleshooting (3 manuals,
  24 sections) compiled with `include_str!`.
- BM25 retrieval with heading boost and a relative-score cutoff.
- Ratatui TUI with red, green, and amber night palettes.
- Streaming generation to the TUI buffer; `--ask` streams to stdout.
- `--ctx` / `FIELD_EVAL_CTX` to trade context window for memory.
- `--selftest` retrieval regression check (6 cases).
- `scripts/build.sh` encoding the four host-specific toolchain traps.

### Verified
- Retrieval selftest 6/6.
- Generation with Qwen2.5-3B Q4_K_M: 58 s per short cited answer at ctx 1024,
  ~2 GB RSS.
- TUI renders and exits cleanly under a PTY.

### Known limits
- CPU only; the Vulkan feature cannot work in this rootfs (no loader/ICD,
  `/dev/dri` denied).
- One resident model per device; a second concurrent run is OOM-killed
  without a log line.
- Corpus is authored reference material, not medical or mechanical authority.
- TUI interaction is keyboard-only; no mouse support.
