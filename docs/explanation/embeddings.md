# Embeddings in the Rust daemon

Semantic and hybrid `search` rank chunks by how close their embedding is to the query's. The TS CLI this one replaced made embeddings with Transformers.js, which ran `Xenova/all-MiniLM-L6-v2` at revision `751bff37…` (its 8-bit quantised ONNX file, `dtype: q8`) on onnxruntime-node 1.21. It mean-pooled the token vectors and normalised the result to 384 dimensions, 16 texts per call.

The Rust daemon runs the same ONNX file with [tract](https://github.com/sonos/tract), a pure-Rust inference engine, one text at a time, in a low-priority worker process. This page records why, with the measurements behind it, taken while both CLIs existed (the TS sources and scripts are in tag `v0.1.5`).

## The choice

| | ort 2.0.0-rc.13 (ONNX Runtime 1.28, q8 ONNX) | candle 0.11 (fp32 safetensors) | **tract 0.23.8 (q8 ONNX)** |
|---|---|---|---|
| Builds for `x86_64-apple-darwin` | **No.** No prebuilt runtime for Intel Macs (rc.12 has none either) | Yes | Yes |
| Builds for `aarch64-apple-darwin` | Yes. The build script downloads a static ONNX Runtime from cdn.pyke.io | Yes | Yes |
| Cosine to TS, 16 per batch on both sides (min / mean / under 0.99) | 0.9919 / 0.9966 / 0 of 50 | 0.9816 / 0.9903 / 21 of 50 | 0.9922 / 0.9966 / 0 of 50 |
| Cosine to TS, 1 per batch on both sides | 0.9925 / 0.9978 / 0 | 0.9842 / 0.9912 / 13 | 0.9925 / 0.9980 / 0 |
| Chunks per second, 1 per batch | 85.6 (2 threads) | 23.9 (2 threads) | 27.7 (1 thread), 32.7 (2 threads) |
| Peak memory, 1 per batch | 113 MB | 325 MB | 121 MB |
| Peak memory, 16 per batch | 582 MB | 1,314 MB | 682 MB (674 MB on 2 threads) |
| Size added to the binary (stripped) | +18.0 MB | +1.2 MB | +23.0 MB |
| Model files | The TS CLI's (23 MB, often already cached) | Another repository's fp32 weights (91 MB) | The TS CLI's |

tract is the only candidate that builds for both release targets with `cargo build --locked` and no system libraries, and its vectors agree with the TS CLI's to a cosine of at least 0.99. candle builds anywhere, but it runs the unquantised weights, so its vectors sit further from the quantised model's than the 0.99 bar. ort is the fastest, but it ships no ONNX Runtime for Intel Macs. Building one from source needs CMake and a long CI step; loading it at run time needs a library installed beside the binary, which `install.sh` doesn't do.

tract costs speed and size. 28 chunks a second embeds BK's 47,000 chunks in about half an hour once, and new chats in seconds after that. The release binary grows from 21.2 MB to 46.3 MB (35.0 MB stripped).

## How it was measured

On 2026-10-02, on BK's Apple-silicon Mac (12 performance and 4 efficiency cores), with release builds and the model at max length 512:

- 50 real chunks, sampled from the TS index by a fixed hash of their ids (92 to 837 characters), embedded as the TS CLI embeds them: `title + "\n" + body`.
- TS vectors from `scripts/dump-embeddings.ts` (in tag `v0.1.5`), which ran the TS CLI's own `LocalEmbedder`.
- Each candidate in a throwaway crate, 500 embeddings for speed and one process each for peak memory (`/usr/bin/time -l`); `Cargo.toml` and the sizes above came from single-backend builds.

Neither the texts nor the vectors are kept.

### The TS CLI doesn't agree with itself past 0.99

The quantised model scales each batch's activations as a whole (`DynamicQuantizeLinear`), padding included. A text's vector therefore depends on which texts share its batch:

| Comparison | min | mean |
|---|---|---|
| TS vectors in BK's index vs the same texts embedded again by TS (other batch-mates) | 0.9894 | 0.9935 |
| TS, 16 per batch vs 1 per batch | 0.9845 | 0.9929 |

Rust chunk ids, and so batch-mates, never match the TS CLI's, so no runtime could reproduce its stored vectors exactly. With one text per batch on both sides, tract matches TS at a mean of 0.998; what's left is the difference between ONNX Runtime 1.21 and tract (ort 1.28 lands within 0.0002 of tract). The daemon embeds one text at a time. That makes each vector depend on its own text only, so a run that stops and resumes produces the same vectors. It's faster than batches of 16, since there's no padding, and it peaks at 121 MB instead of 682 MB. Search queries are embedded one at a time in both CLIs, so query vectors match TS's as closely as anything can.

### Tokenising

The Rust `tokenizers` crate gives the same token ids as Transformers.js for all 50 texts, once one setting is overridden. The model's `tokenizer.json` asks for truncation at 128 tokens and padding to 128; Transformers.js ignores both and truncates at `model_max_length` (512). At 128, cosine to TS falls to a mean of 0.929.

### Intel Macs

The `x86_64-apple-darwin` build, run under Rosetta, gives the same vectors (mean 0.9977, min 0.9925 against TS) at 5.2 chunks a second in emulation. Native Intel speed wasn't measured.

## Bounding CPU

The mxr daemon once pegged four cores with a background model. Here the model runs in a worker process, `chatgpt daemon embed-worker`, which the daemon starts under `taskpolicy -c utility` (or `nice -n 10` where `taskpolicy` is missing), on a single thread. Lowering one thread's priority on macOS needs `unsafe` calls, which this workspace forbids; a process can be started at low priority instead. The worker uses at most one core. It's stopped after ten idle minutes, which frees its memory.

| Policy for the worker | Chunks per second |
|---|---|
| none | 27.8 |
| `nice -n 10` | 27.8 (nice only matters when cores are scarce) |
| `taskpolicy -c utility` (used) | 18.3 |
| `taskpolicy -c background` | 5.1 (efficiency cores only) |

Utility costs a third of the speed when the machine is idle, and gives way to anything the user runs.

On BK's account (2026-10-02, a fresh instance and an empty model cache), the daemon downloaded the model in seconds and embedded all 47,415 chunks of 811 chats in about 31 minutes, at 25 to 28 chunks a second. The worker held one core (94 to 99%) at scheduling priority 20 (the daemon's is 31) in 92 to 139 MB; the daemon itself stayed under 6%. Semantic search then answered in 0.11 s, against the TS CLI's 0.35 s. Over 12 queries, Rust's top five matched the TS CLI's in 93% of places for `--semantic` (the same first result in 11) and 98% for `--hybrid` (10). The differences were near ties.

## The model files

| File | Size | SHA-256 |
|---|---|---|
| `onnx/model_quantized.onnx` | 22,972,370 | `afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1` (Hugging Face's LFS id for the file) |
| `tokenizer.json` | 711,661 | `da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0` |

They come from `https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/751bff37182d3f1213fa05d7196b954e230abad9/<file>` and are cached in `$XDG_CACHE_HOME/chatgpt-cli/models/Xenova/all-MiniLM-L6-v2/<revision>/`, else `~/.cache/…` (where the TS CLI cached them, so a machine that ran it downloads nothing). The daemon downloads a missing or damaged file in the background, checks its size and checksum, and only then moves it into place. The worker checks the checksums again when it loads the model.

A failed download (offline, a proxy, a changed file) never stops lexical search. `chatgpt daemon status` shows why embedding waits, a semantic search says the same, and the daemon tries again 15 minutes later on its own. `chatgpt search-index` tries at once.

## Versions

Vectors carry the `MODEL_VERSION` that made them (`crates/embed/src/lib.rs`). The Rust one is the TS CLI's with `:tract-batch1` added, since the two runtimes' vectors are close but not equal. Change the Rust `MODEL_VERSION` with the model, its revision, its pooling, the runtime or the batch size; the daemon then embeds every chunk again in the background.
