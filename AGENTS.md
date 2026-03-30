# Hark ML — Agent Guide

## Project Structure

```
hark/
├── hark/                          # macOS app (Swift, Xcode)
│   ├── hark.xcodeproj
│   ├── Services/
│   │   ├── AudioRecorder.swift    # Mic input, resampling, pre-buffer
│   │   └── TranscriptionService.swift  # Swift FFI wrapper for Rust ASR
│   └── AppState.swift
├── qwen3-asr-rs/                  # Qwen3 ASR engine (Rust)
│   ├── src/
│   │   ├── streaming.rs           # Streaming ASR with VAD gate
│   │   ├── inference.rs           # Batch ASR inference
│   │   ├── forced_aligner.rs      # Word-level forced alignment
│   │   └── encoder.rs             # Incremental audio encoder
│   ├── qwen3-asr-ffi/             # C FFI for Swift integration
│   │   ├── src/lib.rs
│   │   └── include/qwen3_asr_ffi.h
│   └── examples/live.rs           # Live mic demo
├── asr-synth/                     # ML training pipeline workspace
│   ├── corpus.db                  # SQLite database (vocab, sentences, corpus pairs, jobs)
│   ├── data/                      # Generated data files
│   │   └── corpus_dashboard.jsonl # Exported for training (generated from DB)
│   ├── training/
│   │   ├── data/                  # train.jsonl, valid.jsonl, test.jsonl
│   │   └── adapters/              # LoRA adapter weights
│   ├── voices/
│   │   └── amos2_short.wav        # Voice clone reference (15s, 24kHz mono)
│   └── crates/
│       ├── synth-dashboard/       # Main web dashboard (Rust/Axum + React SPA)
│       │   ├── src/
│       │   │   ├── main.rs        # Server, routes, AppState
│       │   │   ├── jobs.rs        # All job logic: corpus gen, train, eval, vocab scan
│       │   │   ├── review.rs      # Sentence/vocab review with precompute
│       │   │   ├── db.rs          # SQLite schema, migrations, queries
│       │   │   └── tts.rs         # TTS manager, pocket-tts pool, OpenAI, audio encoding
│       │   └── static/index.html  # Single-file React SPA (Babel, no build step)
│       ├── synth-train/           # Training utilities (MLX-LM wrapper)
│       │   └── src/lib.rs         # TrainConfig, InferenceServer, SentenceGenerator, prepare()
│       ├── synth-textgen/         # Text generation (Markov chains, vocab extraction)
│       │   └── src/
│       │       ├── templates.rs   # Sentence generation, extract_sentences()
│       │       └── corpus.rs      # Vocab extraction, pronunciation overrides
│       └── pocket-tts/            # Vendored TTS (NOT used as path dep, just source reference)
```

## Key Commands

### Running the dashboard

```bash
cd ~/bearcove/hark/asr-synth

# Dev build + run (default: localhost:3456, 2 TTS workers)
cargo run -p synth-dashboard -- --voice voices/amos2_short.wav

# With options
cargo run -p synth-dashboard -- \
  --voice voices/amos2_short.wav \
  --host 0.0.0.0 \           # Listen on all interfaces (for remote access)
  --port 3456 \
  --tts-workers 2 \           # pocket-tts parallel workers
  --qwen-model ~/Library/Caches/qwen3-asr/Alkd--qwen3-asr-gguf--qwen3_asr_1_7b_q8_0_gguf

# Release build on souffle (remote Mac Studio)
cargo build -p synth-dashboard --release
./target/release/synth-dashboard --voice voices/amos2_short.wav --host 0.0.0.0
```

### Building Hark (macOS app)

```bash
cd ~/bearcove/hark

# 1. Build the Rust FFI library first
cd qwen3-asr-rs && cargo build --release -p qwen3-asr-ffi && cd ..

# 2. Symlink to where Xcode expects it
mkdir -p ~/bearcove/qwen3-asr-rs/target/release
ln -sf ~/bearcove/hark/qwen3-asr-rs/target/release/libqwen3_asr_ffi.a \
       ~/bearcove/qwen3-asr-rs/target/release/libqwen3_asr_ffi.a

# 3. Build with Xcode
xcodebuild -project hark.xcodeproj -scheme hark -configuration Release -derivedDataPath build

# 4. Install
cp -R build/Build/Products/Release/hark.app /Applications/hark.app
```

### Running tests

```bash
cd ~/bearcove/hark/asr-synth
cargo nextest run -p synth-dashboard    # 7 trim tests
cargo nextest run -p synth-train        # training tests

cd ~/bearcove/hark/qwen3-asr-rs
cargo test --lib streaming::tests       # 38 streaming/VAD tests
```

### Syncing to souffle (Mac Studio)

```bash
cd ~/bearcove/hark

# Using the sync script (excludes target, .git, corpus.db)
echo "y" | bash sync-to-souffle.sh

# Frontend-only deploy for synth-dashboard UI changes
bash deploy-frontend-to-souffle.sh

# Manual rsync (ALWAYS exclude corpus.db to avoid overwriting souffle's DB)
rsync -av --exclude target --exclude .git --exclude 'corpus.db*' \
  ~/bearcove/hark/ souffle:~/bearcove/hark/

# Sync model caches
rsync -av ~/Library/Caches/qwen3-asr/ souffle:~/Library/Caches/qwen3-asr/
```

## Architecture Overview

### Pipeline: Import → Vocab → Review → Corpus → Train → Eval

1. **Import**: Scan JSONL chat history (hark/claude/codex) incrementally, extract vocab + sentences
2. **Vocab**: Browse, search, LLM-curate (GPT-4o-mini), add pronunciation overrides
3. **Review**: Approve/reject vocab terms with TTS preview + dual ASR
4. **Corpus**: Generate training pairs:
   - LLM (Qwen2.5-1.5B-Instruct) generates natural sentences containing each term
   - Falls back to Markov chain if LLM unavailable
   - TTS speaks the sentence (pocket-hq or OpenAI)
   - Dual ASR (Qwen3 + Parakeet) transcribes
   - Forced alignment + tri-boundary consensus extracts the term fragment
   - Stores (original, qwen_heard, parakeet_heard) triplets in SQLite
5. **Train**: LoRA fine-tune Qwen2.5-0.5B via MLX-LM with early stopping
6. **Eval**: Compare pre-correction vs post-correction error rates on corpus pairs

### Key Algorithms

**Tri-boundary consensus** (corpus extraction):
1. Align original text, Qwen ASR output, and Parakeet ASR output against the same audio
2. Compute boundaries per lane (word start + end times)
3. Find tri-boundaries: times where all 3 lanes agree within 50ms
4. Annotate each tri-boundary with the word before/after in each lane
5. For left boundary: walk right-to-left, pick first where `before` words match across lanes
6. For right boundary: walk left-to-right, pick first where `after` words match across lanes
7. Extract words whose start_time falls within [left, right)

**Gap expansion** (before boundary finding):
- After finding the term in the original alignment, expand left/right into silence gaps > 50ms
- This captures ASR words that bleed into the gaps (e.g., "lldb" → "L" + "LDB")

**Edge trimming**:
- After extraction, trim matching words from left/right edges
- A word is trimmed only if it matches across all active lanes (case-insensitive)
- Protected terms (the target vocab word) are never trimmed

### Database (corpus.db)

Key tables:
- `vocab`: Terms with pronunciation overrides, curated/reviewed flags, descriptions
- `corpus_pairs`: Training triplets with alignment data, audio (Ogg Opus BLOB), hit counts
- `sentences` / `candidate_sentences`: Imported sentences
- `jobs`: Job tracking (corpus gen, train, eval, etc.)
- `import_offsets`: Per-source file byte offsets for incremental import
- `vocab_confusions`: ASR error patterns from vocab scan

### TTS Pool

pocket-tts runs as a pool of N workers sharing model weights via Arc:
- Each worker has its own voice_state (attention caches)
- `generate()` picks a free worker via try_lock
- Workers isolated on separate Metal devices for GPU concurrency
- Configurable via `--tts-workers N` (default 2)

### Sentence Generator (LLM)

Corpus generation uses a local Qwen2.5-1.5B-Instruct model:
- Wraps `mlx_lm.server` on port 8890
- Chat-style prompt: "Generate a natural sentence containing the exact token: {term}"
- Validates output contains the term, rejects echo-backs, retries up to 3 times
- Falls back to Markov chain if LLM fails

### Environment Variables

- `OPENAI_API_KEY` — enables OpenAI TTS backend
- `ELEVENLABS_API_KEY` — enables ElevenLabs TTS backend
- `HF_TOKEN` — HuggingFace token for pocket-tts model download

### Ports

- `3456` — Dashboard web UI
- `8890` — Sentence generator (mlx_lm.server, Qwen2.5-1.5B-Instruct)
- `8899` — Correction inference server (mlx_lm.server, Qwen2.5-0.5B + LoRA adapters)

### Frontend (static/index.html)

Single-file React SPA using Babel transform (no build step). Hash routing:
- `#/` or `#/vocab` — Vocabulary management
- `#/author` — Author/generate sentences
- `#/corpus` — Corpus generation + training data prep
- `#/train` — LoRA training with early stopping
- `#/eval` — Pre vs post correction error rates
- `#/tests` — Visual algorithm test suite (19 synthetic edge cases)
- `#/jobs` — Job history
- `#/review/active` — Active vocab review session
