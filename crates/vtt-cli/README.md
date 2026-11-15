# `vtt-trans`

`vtt-trans` translates WebVTT with a two-stage local LLM pipeline. It builds one `master_glossary.json` from caption files using the Stage 2 model, then shares that glossary across Stage 1 and Stage 2 and across episode checkpoints. `--vocab` is optional; when supplied, its entries are merged into the master glossary and override matching generated terms.

## 1. Build the master glossary

The default Stage 2 model alias is `ERNIE-4.5-21B-A3B-Q4_K_M`. Make the matching GGUF available at `models/ERNIE-4.5-21B-A3B-Q4_K_M.gguf`, then start its server:

```powershell
llama-server -m models/ERNIE-4.5-21B-A3B-Q4_K_M.gguf --alias ERNIE-4.5-21B-A3B-Q4_K_M --threads 16 --ctx-size 8192 --cache-type-k q8_0 -ngl 0 --host 127.0.0.1 --port 8080
```

Build the glossary from one or more caption files. Directories are scanned recursively for `.vtt` files; repeated warning captions are ignored.

```powershell
cargo run --release -p vtt-cli -- --stage build-glossary --caption-file in\.tmp_S02E001\captions.zh.vtt --caption-file in\.tmp_S02E002\captions.zh.vtt --work-dir work\series --genre "huyền huyễn, tu tiên" --stage2-base-url http://127.0.0.1:8080/v1
```

The result is `work\series\master_glossary.json`. It contains genre-specific address/style rules and caption-derived character names, aliases, titles, places, and terms. You can also pass a caption directory:

```powershell
cargo run --release -p vtt-cli -- --stage build-glossary --caption-file in\.tmp_S02E001 --work-dir work\series --genre "huyền huyễn, tu tiên" --stage2-base-url http://127.0.0.1:8080/v1
```

Genre examples:

```powershell
cargo run --release -p vtt-cli -- --stage build-glossary --caption-file captions.zh.vtt --work-dir work\xianxia --genre "huyền huyễn, tu tiên" --stage2-base-url http://127.0.0.1:8080/v1
cargo run --release -p vtt-cli -- --stage build-glossary --caption-file captions.zh.vtt --work-dir work\modern-drama --genre "hiện đại, drama" --stage2-base-url http://127.0.0.1:8080/v1
cargo run --release -p vtt-cli -- --stage build-glossary --caption-file captions.zh.vtt --work-dir work\90s-drama --genre "thập niên 90, drama" --stage2-base-url http://127.0.0.1:8080/v1
```

If translation starts with `--caption-file` and the master glossary does not exist (or its captions/genre changed), the CLI builds it automatically through the Stage 2 endpoint. To keep model use sequential and reduce memory pressure, build the glossary first, stop ERNIE, and then start the Stage 1 server.

## 2. Run Stage 1 and Stage 2

Start Hy-MT2 for Stage 1:

```powershell
llama-server -m models/Hy-MT2-30B-A3B-Q4_K_M.gguf --alias Hy-MT2-30B-A3B --threads 16 --ctx-size 4096 --cache-type-k q8_0 -ngl 0 --host 127.0.0.1 --port 8081
```

Run Stage 1 with the same `--work-dir` and `--genre` used to create the glossary:

```powershell
cargo run --release -p vtt-cli -- --stage stage1 --input in\S02E001.merged.zh.vtt --work-dir work\series --genre "huyền huyễn, tu tiên" --stage1-base-url http://127.0.0.1:8081/v1 --stage1-model Hy-MT2-30B-A3B
```

Stop Hy-MT2, start ERNIE on port 8082, then resume Stage 2:

```powershell
llama-server -m models/ERNIE-4.5-21B-A3B-Q4_K_M.gguf --alias ERNIE-4.5-21B-A3B-Q4_K_M --threads 16 --ctx-size 8192 --cache-type-k q8_0 -ngl 0 --host 127.0.0.1 --port 8082
cargo run --release -p vtt-cli -- --stage stage2 --work-dir work\series --output output\S02E001.vi.vtt --genre "huyền huyễn, tu tiên" --stage1-model Hy-MT2-30B-A3B --stage2-base-url http://127.0.0.1:8082/v1
```

The `--genre` must match the cached glossary. Stage 1 and Stage 2 default to batches of 20 and 35 cues; override with `--stage1-batch-size` or `--stage2-batch-size`. Stage 2 prints a warning for any cue whose final text does not contain a required caption-derived character, title, or place translation.

To let the CLI build the glossary automatically, add one or more `--caption-file PATH` arguments to a Stage 1 or `all` command and keep the Stage 2 endpoint available. `--vocab vocabulary.txt` remains an optional manual override, not a requirement.

## Directory input

Directory input processes top-level source `.vtt` files in name order, skips existing `.vi.vtt` files, creates a checkpoint per episode under `<work-dir>/<input-directory>/<file-stem>/`, and writes translations under `--output-dir`. Caption directories, unlike input directories, are scanned recursively.

```powershell
cargo run --release -p vtt-cli -- --stage stage1 --input in --work-dir work\series --genre "huyền huyễn, tu tiên" --stage1-base-url http://127.0.0.1:8081/v1
cargo run --release -p vtt-cli -- --stage stage2 --input in --work-dir work\series --output-dir output\in --genre "huyền huyễn, tu tiên" --stage2-base-url http://127.0.0.1:8082/v1
```
