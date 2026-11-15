# `vtt-trans`

`vtt-trans` translates WebVTT using OpenAI-compatible local chat-completion servers. The default stage is `all`; `stage1` and `stage2` can run separately so the first model can be stopped before the second is loaded.

## Run Stage 1, switch models, then run Stage 2

Start the Stage 1 model in **Terminal 1**. Adjust `--threads` to the CPU and `--ctx-size` to the available memory:

```powershell
llama-server -m models/Hy-MT2-30B-A3B-Q4_K_M.gguf --alias Hy-MT2-30B-A3B --threads 16 --ctx-size 8192 --cache-type-k q8_0 -ngl 0 --host 127.0.0.1 --port 8080
```

Run the CLI in **Terminal 2**:

```powershell
cargo run --release -p vtt-cli -- --stage stage1 --input input/movie.vtt --work-dir work --vocab vocabulary.txt --stage1-base-url http://127.0.0.1:8080/v1 --stage1-model Hy-MT2-30B-A3B
```

After Stage 1 finishes, stop `llama-server` in Terminal 1 and start it again with the Stage 2 model (the same port can be reused):

```powershell
llama-server -m models/Qwen3-30B-A3B-Q4_K_M.gguf --alias Qwen3-30B-A3B --threads 16 --ctx-size 8192 --cache-type-k q8_0 --reasoning-budget 0 -ngl 0 --host 127.0.0.1 --port 8080
```

Then resume from the checkpoint in Terminal 2:

```powershell
cargo run --release -p vtt-cli -- --stage stage2 --work-dir work --output output/movie.vi.vtt --vocab vocabulary.txt --stage2-base-url http://127.0.0.1:8080/v1 --stage2-model Qwen3-30B-A3B --stage1-model Hy-MT2-30B-A3B
```

When using a custom Stage 1 model name, pass the same `--stage1-model` value to both commands so the checkpoint fingerprint matches. The Stage 2 command reads `source.vtt` and `checkpoint.json` from `--work-dir`; it does not need `--input`.

Stage 2 disables Qwen thinking in each request with `chat_template_kwargs.enable_thinking=false` and a `/no_think` system-prompt suffix. Keep `--reasoning-budget 0` on the Qwen llama-server command as an additional server-side safeguard.

To run both stages without switching servers:

```powershell
cargo run --release -p vtt-cli -- --input input/movie.vtt --output output/movie.vi.vtt --work-dir work --vocab vocabulary.txt --stage1-base-url http://127.0.0.1:8080/v1 --stage2-base-url http://127.0.0.1:8080/v1
```

For a directory, the CLI processes top-level source `.vtt` files in name order (other files and existing `.vi.vtt` translations are ignored). Each checkpoint is kept under `work/<input-directory>/<file-stem>/`; `--output-dir` receives `<file-stem>.vi.vtt`:

```powershell
cargo run --release -p vtt-cli -- --stage stage1 --input in --work-dir work --vocab vocabulary.txt --caption-file captions.zh.vtt --stage1-batch-size 20 --stage2-batch-size 35 --stage1-base-url http://127.0.0.1:8080/v1
cargo run --release -p vtt-cli -- --stage stage2 --input in --work-dir work --output-dir output/in --vocab vocabulary.txt --stage2-base-url http://127.0.0.1:8080/v1
```

Use the same vocabulary file and Stage 1 model name in both stages; its fingerprint is checked when resuming.
Repeat `--caption-file` for additional caption files, or pass directories to scan recursively. Generated name/title entries are cached once at `<work-dir>/auto_glossary.json` and reused for all per-file checkpoints; manual `vocabulary.txt` entries take precedence. Stage 1 and Stage 2 default to batches of 20 and 35 cues respectively; use `--stage1-batch-size` and `--stage2-batch-size` to override them.

## Ollama

Run `ollama serve` in **Terminal 1**. Both models must already be available (import the GGUF models first if needed). The OpenAI-compatible endpoint is typically `http://127.0.0.1:11434/v1`:

```powershell
ollama serve
```

In **Terminal 2**, run Stage 1, unload its model after the batch completes, then resume with Stage 2:

```powershell
ollama list
cargo run --release -p vtt-cli -- --stage stage1 --input input/movie.vtt --work-dir work --vocab vocabulary.txt --stage1-base-url http://127.0.0.1:11434/v1 --stage1-model Hy-MT2-30B-A3B
ollama stop Hy-MT2-30B-A3B
cargo run --release -p vtt-cli -- --stage stage2 --work-dir work --output output/movie.vi.vtt --vocab vocabulary.txt --stage2-base-url http://127.0.0.1:11434/v1 --stage2-model Qwen3-30B-A3B --stage1-model Hy-MT2-30B-A3B
```

The CLI displays cue progress and ETA on stderr. The final summary reports translated cues, cues changed by Stage 2, and cues that fell back to the Stage 1 draft after an unsafe or duplicate edit; omitted Stage 2 edits are valid unchanged drafts, not fallback errors. Server compatibility depends on support for the OpenAI chat-completions endpoint, the request sampler fields, and Stage 2 JSON Schema response format.
