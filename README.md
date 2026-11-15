# VTT Translator

`vtt-trans` is a Rust CLI that translates WebVTT subtitles through two sequential stages: Hy-MT drafts the translation, then Qwen reviews it for meaning and consistency. The workspace contains four crates: `vtt-core`, `llm-client`, `translator-engine`, and `vtt-cli`.

## Requirements

- Rust toolchain compatible with edition 2024.
- `hymt_bin/llama-server.exe` and its runtime DLLs.
- GGUF models in `models/`: Hy-MT for Stage 1 and Qwen for Stage 2.
- A glossary file such as `vocabulary.txt`.

## Translate a directory

Start the Stage 1 model in one terminal:

```powershell
.\hymt_bin\llama-server.exe -m .\models\Hy-MT2-30B-A3B-Q4_K_M.gguf -ngl 0 -c 4096 --cache-type-k q8_0 --host 127.0.0.1 --port 8081 --alias Hy-MT2-30B-A3B
```

In another terminal, draft translations for top-level `.vtt` files in `in/`:

```powershell
cargo run --release -p vtt-cli -- --stage stage1 --input in --work-dir work --vocab vocabulary.txt --stage1-model Hy-MT2-30B-A3B --stage1-base-url http://127.0.0.1:8081/v1
```

Stop the first server and start Qwen:

```powershell
.\hymt_bin\llama-server.exe -m .\models\Qwen3-30B-A3B-Q4_K_M.gguf -ngl 0 -c 8192 --cache-type-k q8_0 --host 127.0.0.1 --port 8082 --alias Qwen3-30B-A3B
```

Resume Stage 2 from those checkpoints and write translated files into `output/in/`:

```powershell
cargo run --release -p vtt-cli -- --stage stage2 --input in --work-dir work --output-dir output/in --vocab vocabulary.txt --stage1-model Hy-MT2-30B-A3B --stage2-model Qwen3-30B-A3B --stage2-base-url http://127.0.0.1:8082/v1
```

Each source receives its own checkpoint directory under `work/`. Directory mode processes top-level `.vtt` files in name order and ignores non-VTT files and existing `.vi.vtt` translations. For a single input, use `--input source.vtt --output output/translated.vi.vtt`; omit `--stage` to run both stages against the configured endpoints.

The CLI applies only glossary entries present in each batch. Use the same `--vocab` file and `--stage1-model` value when resuming Stage 2.

## Workspace checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [docs/](docs/) for the WebVTT format, architecture, and pipeline details. The CLI's server and checkpoint options are documented in [crates/vtt-cli/README.md](crates/vtt-cli/README.md).
