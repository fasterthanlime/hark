# beeml Correction System

## Purpose

This document is the canonical roadmap for the correction product that `beeml`
and `beeml-web` are supposed to become.

Related supporting docs:

- [phonetic-retrieval-implementation-checklist.md](/Users/amos/bearcove/bee/docs/phonetic-retrieval-implementation-checklist.md)
- [eval-frontend-handoff.md](/Users/amos/bearcove/bee/docs/eval-frontend-handoff.md)

Those documents cover narrower slices. This one defines the whole shape:

- what is being trained
- what runs in production
- what assets are canonical
- what `beeml` should expose
- what `beeml-web` must make debuggable

## Current Reality

Today:

- `beeml` is still a minimal transcription RPC server
- `beeml-web` is still a minimal transcription demo with some richer inspector
  scaffolding
- `bee-phonetic` now has the first file-first seed dataset and a first indexed
  retrieval baseline

What is already effectively solved enough for this design:

- ASR
- forced alignment

What is not solved:

- retrieving good correction candidates fast enough
- deciding which local correction to apply using sentence context
- making the whole decision path inspectable in the frontend

That means the product is not "transcription". The product is:

- transcript correction for technical vocabulary
- phonetic retrieval and verification
- contextual final judging
- interactive inspection and evaluation

## Final Objective

The end state should be:

- `beeml` is the production and evaluation backend for correction
- `beeml-web` is the main frontend for live debugging, evaluation, and review
- the correction system is retrieval-first, not generation-first
- every accepted or rejected correction is explainable in the UI without reading
  logs

The system should accept an already-good transcript plus timings, retrieve a
small set of plausible span-local corrections, verify them phonetically, choose
among them with a contextual final judge, and return both the corrected text
and the reasoning trace.

## System Split

The architecture should be treated as two different systems sharing assets:

1. training and evaluation
2. production inference

They use the same lexicon and retrieval concepts, but they have different
responsibilities.

## Training and Evaluation

Training exists to build and validate correction artifacts.

It should answer:

- which aliases and pronunciations belong in the lexicon
- which retrieval views actually recover the target term
- whether the final judge can choose correctly when the target is present
- which artifact bundle should be promoted into production

### Training Inputs

The durable source data should be file-first and reviewable.

Primary sources today:

- [data/phonetic-seed/vocab.jsonl](/Users/amos/bearcove/bee/data/phonetic-seed/vocab.jsonl)
- [data/phonetic-seed/sentence_examples.jsonl](/Users/amos/bearcove/bee/data/phonetic-seed/sentence_examples.jsonl)
- [data/phonetic-seed/recording_examples.jsonl](/Users/amos/bearcove/bee/data/phonetic-seed/recording_examples.jsonl)
- [data/phonetic-seed/audio](/Users/amos/bearcove/bee/data/phonetic-seed/audio)

Confusion surfaces should not be part of the default canonical seed until they
prove useful enough to justify the noise.

### Data Split and Leakage Policy

Evaluation hygiene must be explicit.

At minimum, experiments should avoid leakage across:

- the same canonical term
- alias families of the same term
- near-duplicate sentence contexts
- the same recording speaker or session
- user-personalized data versus base-bundle training data

For online-learning evaluation, random holdout alone is not enough.

The default comparison regime should include:

- term-family-isolated evaluation
- speaker or session isolation where applicable
- time-aware evaluation that simulates user history when judging online updates

Random splits are acceptable as smoke tests. They should not be the only
headline number.

### Training Products

Training should produce explicit, versioned artifacts, not opaque database
state.

Expected artifact families:

- normalized lexicon snapshot
- derived alias snapshot
- phonetic retrieval indexes
- retrieval evaluation fixtures and metrics
- final-judge training examples
- tiny judge weights
- optional neural fallback weights
- bundle metadata and thresholds

### Training Stages

The training/eval loop should be separated into two questions.

#### 1. Retrieval

For a transcript span, does the target term enter the shortlist at all?

This is where we measure:

- top-1 / top-3 / top-10 target recall
- miss buckets by term, span shape, token count, identifier class
- which retrieval view produced the candidate
- where short queries or technical terms fail

If the target never enters the shortlist, the final judge is not at fault.

#### 2. Final Judging

Given a small local candidate set that already contains the target, can the
model choose the right edit while accounting for sentence context?

This is a selection problem, not a free-form generation problem.

The final-judge training examples should look like:

- left context
- original span
- right context
- keep-original option
- candidate replacements
- optional retrieval features and priors
- gold choice

That is the regime where a small model is plausible.

### Judge Scope

The learned part of the system should stay constrained, but it should not be
assumed to be "one reranker model".

The default design should be a three-level judge:

1. deterministic scorer over phonetic and structural features
2. tiny learned scorer over candidate features
3. optional frozen neural fallback for ambiguous cases only

This is the right fit for a product where:

- users should be able to add vocabulary without retraining
- user corrections should update behavior immediately
- vocabulary belongs in lexicon and memory, not in model weights

Do not treat the main learned layer as a general sentence rewrite model.

The intended learned behavior is:

- compare a small number of local alternatives
- consume structured phonetic and context features
- preserve the original when no candidate is better
- improve incrementally from user corrections

That means the central ML question is not only:

- can a 0.5B to 0.6B reranker choose among local alternatives?

It is first:

- can a tiny feature-based judge, updated online, resolve most final decisions
  before a neural fallback is even needed?

## Production Inference

Production inference should load a fixed bundle and serve correction RPCs.

It should not depend on query-heavy database logic.

### Production State Model

Production state should not be treated as one opaque blob.

It should be split into:

1. base bundle
2. user overlay
3. session or project overlay

#### Base Bundle

The base bundle is:

- versioned
- read-only at runtime
- shipped as the canonical product artifact

It should contain:

- reviewed lexicon and aliases
- normalized phonetic views
- retrieval indexes
- deterministic thresholds
- tiny judge weights
- optional neural fallback weights

#### User Overlay

The user overlay is local and mutable.

It should contain:

- user-added vocabulary
- user-added aliases or pronunciation hints
- user-local term priors
- user-local confusion memory
- optional user-local online weights

This is the main personalization surface.

#### Session or Project Overlay

The session or project overlay is ephemeral and cheap to reset.

It should contain:

- recent vocabulary
- repetition priors
- project-local term boosts
- temporary correction context

For retrieval, this implies:

- global retrieval indexes from the base bundle
- small incremental overlay indexes for user and session additions
- merged candidate views at query time

That architecture is better than treating production state as one mutable
artifact.

#### Overlay Precedence and Merge Rules

Overlay behavior should be explicit.

Default precedence should be:

1. session or project overlay
2. user overlay
3. base bundle

This means:

- session or project context may temporarily override user-level priors
- user personalization overrides shipped defaults
- base bundle remains the fallback

At query time, retrieval merge semantics should be explicit too:

- candidates should be deduped by canonical term, not by raw alias row alone
- alias-level evidence should be retained as provenance under the deduped term
- competing priors should merge by precedence first, then by additive evidence
  where that is meaningful
- duplicate alias entries across layers must not double-count overlap or boost
  priors accidentally

Rebuild and replay behavior should also be explicit:

- base bundle is loaded as-is
- overlays are replayed on top
- event log replay must be sufficient to rebuild user and session state from
  scratch

### Production Inputs

The production path should assume ASR and alignment are already available.

The core request should be:

- transcript
- word timings
- optional confidence later
- optional raw audio when needed for debug or fallback features

### Production Pipeline

The correction path should be:

1. receive transcript and timings
2. enumerate plausible contiguous spans
3. derive searchable phonetic views for each span
4. retrieve candidates through indexed lanes
5. verify only the small shortlist with a stronger phonetic scorer
6. judge local alternatives with sentence context and user priors
7. assemble the corrected sentence
8. return both result and trace

### Latency Budgets

The pipeline is interactive, so each stage needs an explicit budget.

The exact numbers can evolve, but the system should measure at least:

- span proposal budget
- retrieval budget
- verifier budget
- final-judge budget
- neural fallback budget
- end-to-end CPU budget

The important rule is:

- optional neural fallback must remain optional in both logic and latency
- each stage should be measured separately so regressions are attributable

### Span and Region Policy

Span proposal and region assembly need to be explicit, not implicit.

This is a real failure surface and should not be blurred into retrieval or
judge behavior.

The system needs a written policy for:

- maximum span length
- punctuation and identifier-boundary handling
- whether adjacent edits may both apply
- whether overlapping candidates compete globally or greedily
- how keep-original interacts with overlapping proposals
- whether final judging is independent per region or subject to a non-overlap
  constraint

At minimum, eval should distinguish:

- span missed
- target not shortlisted given span
- target shortlisted but judged incorrectly
- conflict-resolution or region-assembly failure

### Retrieval Stage

The intended first production-capable retrieval stack is:

- boundary-aware IPA 2-gram postings
- boundary-aware IPA 3-gram postings
- reduced IPA views
- length and token-count filters
- a short-query fallback lane for very short phonetic strings

This is still an inverted-index architecture, but not the weak version where
only raw q-gram overlap is available.

The current low-recall baseline is useful because it confirms the failure mode:

- the target usually does not enter the candidate pool at all

That means the shortlist generator needs more structure, not just a different
final verifier.

The next retrieval upgrade after the raw/reduced baseline is:

- articulatory-feature n-gram postings

That should be treated as the next major retrieval enhancement, not as already
assumed v1 behavior.

### Lexicon and Alias Policy

The retrieval system should index aliases, not just canonical spellings.

Alias families should eventually include:

- canonical term
- human-entered spoken variants
- identifier verbalizations
- carefully accepted confusion-derived variants
- later, optional G2P N-best variants with priors

For technical vocabulary, identifier verbalization is important enough to treat
as first-class:

- camelCase
- snake_case
- digit expansions
- acronym and spelled-letter forms
- symbol verbalizations

This should be represented explicitly rather than hidden in ad hoc aliases.

### Verification Stage

The verifier should be stronger than the retriever and should only run on a
small shortlist.

Its job is:

- reject cheap retrieval noise
- score phonetic plausibility more faithfully
- preserve enough structured evidence for the final judge and UI

The verifier should move toward feature-aware or learned weighted alignment.

It should not be expected to resolve every near-neighbor case that is really a
context or memory problem.

### Final Judge Stage

The final judge should operate region-by-region.

Its choice set should contain:

- keep original
- candidate sentence with candidate edit A
- candidate sentence with candidate edit B
- candidate sentence with candidate edit C

It should not score arbitrary full-sentence rewrites independently.

The default sequence should be:

1. deterministic acceptance or rejection for obvious cases
2. tiny learned scoring over candidate features
3. optional frozen neural reranker only for close margins

The judge output should include:

- chosen candidate index
- chosen text
- deterministic acceptance score
- tiny learned score
- neural fallback score when used
- confidence or margin

### Decision Policy and Abstention

The architecture also needs an explicit decision policy.

At minimum, it should define:

- when deterministic acceptance is enough
- when the tiny learned judge must be consulted
- when neural fallback is allowed
- when the system should abstain and keep the original
- how confidence or margin is interpreted

The intended default should be:

1. deterministic layer accepts or rejects obvious cases
2. tiny learned judge resolves normal ambiguous cases
3. neural fallback runs only when margins remain close
4. if no layer is sufficiently confident, the system abstains and keeps the
   original

Bad corrections will often be abstention failures rather than simple ranking
failures, so this policy must be explicit.

Confidence and margin values should not be treated as trustworthy by default.
They need calibration and should be evaluated as calibrated decision signals,
not just as ranking byproducts.

The multi-edit coordination question belongs to span and region assembly, not
to the judge in isolation. The decision policy should therefore reference the
non-overlap and conflict rules defined in `Span and Region Policy`.

### Shadow and Suggestion Modes

Before aggressive auto-application, the product should support a shadow or
suggestion mode.

In that mode, the system should:

- run the full correction pipeline
- log traces and candidate decisions
- surface suggested edits and abstentions
- avoid silently replacing text unless confidence is already high enough

This helps with:

- safe rollout
- trust
- bundle comparison
- abstention policy tuning
- data collection before stronger automatic application

## Canonical Assets

The system should distinguish between source-of-truth assets and derived
artifacts.

### Source-of-Truth Assets

These should stay reviewable and file-first:

- vocabulary terms
- reviewed IPA
- spoken variants
- authored sentences
- recording manifests
- audio references

### Derived Artifacts

These should be rebuilt, not hand-edited:

- normalized alias rows
- identifier verbalizations
- reduced IPA views
- articulatory feature views
- phonetic indexes
- retrieval eval fixtures
- candidate feature exports
- tiny judge training examples
- tiny judge weights
- optional neural fallback weights
- user-local memory state
- user-local online weight state

### Bundle Manifest

Every promoted base bundle should include an explicit manifest.

At minimum it should record:

- bundle id and version
- feature schema version
- retrieval index version
- threshold policy version
- tiny judge weight version
- optional neural fallback descriptor
- compatibility requirements for overlays and traces
- event-log schema version
- training-data provenance or source snapshot ids
- evaluation summary used for promotion

This prevents silent incompatibilities like loading one weight set against a
different feature schema.

### Promotion Policy

Bundle promotion should be explicit, not implied by "an evaluation summary
exists".

Promotion should require, at minimum:

- retrieval recall does not regress on protected term classes
- end-to-end correction accuracy improves on held-out evaluation
- abstention error rate does not worsen beyond threshold
- latency stays within budget
- no regression on a fixed critical-vocabulary suite
- compatibility checks pass for bundle manifest, feature schema, and event-log
  schema

### Event Log

User correction behavior should also be captured as a first-class artifact
family.

Every correction should become a typed event.

The event log itself should be versioned.

Examples:

- user accepted a suggestion
- user rejected a suggestion
- user replaced a span with candidate X
- user inserted a brand new term
- user supplied an alias or pronunciation hint
- user reverted a prior correction

These events should be the source of truth for:

- user memory
- online updates
- later offline training examples
- debugging why behavior changed

The system should prefer:

- explicit correction events

over:

- opaque local state mutation

At minimum, the event log should carry:

- event schema version
- event type
- timestamp
- enough payload to replay state deterministically

### Forgetting and Rollback

The event-log design should support explicit rollback behavior.

That includes:

- forgetting a user-added term
- undoing a bad pronunciation hint
- rolling back online weight changes to a checkpoint
- clearing only the session or project overlay
- replaying the event log from scratch to rebuild state

This is important for product trust and for debugging state drift.

## `beeml` Backend Role

`beeml` should become the RPC boundary for correction.

It should own:

- artifact loading
- retrieval
- verification
- deterministic judging
- tiny learned judging
- optional neural fallback
- correction assembly
- debug trace production
- evaluation-oriented RPCs

It should not push core correction logic into the frontend.

### RPC Design Principle

Every production operation should have a debuggable representation.

That does not require separate code paths. It does require responses that can
include a trace payload when requested.

The system should prefer:

- one canonical correction pipeline
- one fast response shape
- one richer debug shape built from the same internals

### Expected RPC Families

The exact method names can change, but `beeml` should grow toward families like:

#### Production

- `correct_transcript(...)`
- `stream_correct(...)`
- `transcribe_and_correct(...)` only if still operationally useful

#### Debug and Inspection

- `debug_retrieval(...)`
- `debug_correction(...)`
- `inspect_term(...)`
- `explain_candidate(...)`
- `explain_judge(...)`
- `apply_user_correction(...)`

#### Evaluation

- `run_retrieval_eval(...)`
- `run_correction_eval(...)`
- `list_eval_cases(...)`
- `get_eval_case(...)`

The important boundary is not HTTP routes. It is typed RPC methods and payloads.

## `beeml-web` Frontend Role

`beeml-web` should be the primary debug and evaluation surface for the
correction system.

That means the frontend is not just a result viewer. It is the inspection tool
for the entire pipeline.

### Hard Requirement

Every correction decision should be explainable in `beeml-web`.

No stage should require reading backend logs to understand:

- why a target was missed
- why a candidate survived
- why the final judge chose or rejected an edit

### Required Debug Surface

The frontend should expose the pipeline as stages.

#### 1. Input and Span View

Show:

- transcript
- word timings
- selected span
- span text
- span IPA
- reduced IPA
- feature view if available

#### 2. Retrieval View

Show:

- which retrieval lanes fired
- top candidates per lane
- alias source
- matched alias
- q-gram overlap counts
- feature overlap if present
- token-count and length compatibility
- candidates removed by filtering

#### 3. Verification View

Show:

- verifier score
- phonetic alignment or compact comparison
- source metadata
- why the candidate stayed or fell out

#### 4. Final Judge View

Show:

- sentence context
- keep-original option
- sentence alternatives side by side
- chosen candidate
- deterministic acceptance score
- tiny learned score
- neural fallback score when used
- confidence or margin

#### 5. Eval and Batch Analysis

Show:

- retrieval recall summaries
- correction success summaries
- miss buckets
- per-term failure clustering
- side-by-side comparison of bundle versions

This should support questions like:

- show all misses for `AArch64`
- show all cases where retrieval succeeded but final judging failed
- compare retrieval-only versus retrieval-plus-final-judge

## Debug Data Requirements

The system must preserve provenance through all stages.

For each returned candidate, the trace should be able to answer:

- which transcript span produced it
- which alias source produced it
- which index lanes matched it
- which filters it survived
- what verification score it received
- whether it reached the final judge
- whether it was accepted

Without this, tuning becomes guesswork.

## Suggested Backend Trace Shape

The exact Rust types can evolve, but the logical structure should be:

- request metadata
- transcript and timings
- span list
- considered regions
- retrieval per span
- verification per span
- final-judge candidates per chosen region
- rejected candidate sets
- abstained regions and reasons
- accepted edits
- final corrected text
- timing breakdown

That trace should be serializable and stable enough for `beeml-web` to render
without backend-specific ad hoc transformations.

Failure analysis should always be able to answer:

- what was the first failing stage?
- did any downstream stage ever have a chance to recover?

## Non-Goals

For this system definition, do not optimize for:

- whole-sentence generative rewriting as the main correction mechanism
- database-centric runtime query logic
- hiding uncertainty behind one opaque confidence number
- a frontend that only renders final text

## Roadmap

This roadmap should be read as:

1. committed architecture
2. near-term de-risking work
3. deferred productization

It should **not** be read as a waterfall plan where every design detail must be
implemented before the next experiment starts.

### Bucket 1: Committed Architecture

These are the decisions we should treat as settled unless new evidence forces a
change:

- retrieval-first correction, not generation-first correction
- vocabulary lives in bundles and overlays, not in model weights
- phonetics should surface truthful candidates and phonetic plausibility; it
  should not be tuned until it becomes a fake contextual judge
- the default judge stack is:
  1. deterministic scorer
  2. tiny learned feature-based scorer
  3. optional frozen neural fallback
- user corrections should first update memory and overlays, not trigger default
  retraining
- the whole pipeline must stay inspectable in `beeml-web`

These are architecture constraints, not hypotheses.

### Bucket 2: Near-Term De-Risking

This is the actual execution roadmap.

The goal is to prove or falsify the critical hypotheses with the smallest
amount of machinery.

#### 2.1 Finish the Current Phonetic Loop

We already have most of this, but it needs to be made stable enough to feed
later judge work.

Required:

- span proposal recall is measured explicitly
- retrieval, verification, and region-assembly failures are attributable
- the full verifier trace is visible in `beeml-web`, not only the CLI debugger
- known hard cases are inspectable end to end

Exit criteria:

- the current phonetic stack produces trustworthy candidate traces
- the four oracle layers below are operational on at least one stable
  benchmark suite

#### 2.2 Stabilize Candidate Features v1

Before training any judge, define the feature row the judge will consume.

Candidate feature groups:

- retrieval:
  - matched lane
  - q-gram overlap counts per view
  - shortlist rank
  - cross-view support
- verification:
  - token score
  - feature score
  - weighted distances
  - boundary penalties
  - alignment op summaries
- structure:
  - alias source
  - identifier-part count
  - acronym-like
  - has digits
  - snake/camel/symbol-derived
  - span shape match
- context:
  - left context tokens
  - right context tokens
  - surrounding function-word indicators
- memory:
  - user prior for term
  - user prior for alias
  - confusion prior for observed span -> chosen term
  - session/project prior
  - recency / repetition counters

Required:

- one typed Rust feature struct
- one schema version field
- one stable export path from eval/debug runs
- one frontend inspector for the feature row

Exit criteria:

- every verified candidate can be exported as a stable feature record
- the feature record supports oracle attribution across:
  - span proposal
  - retrieval shortlist
  - final-judge selection
  - end-to-end outcome

#### 2.3 Add the Simplest Useful Overlay Memory

Before online learning, prove that memory alone buys value.

Do first:

- user-local term priors
- alias priors
- session/project recency priors
- confusion-memory counts

Do not do yet:

- user-local online weight updates

Required:

- base bundle + overlay loading
- overlay precedence and merge policy
- deterministic replay from event log
- UI visibility into memory contributions

Exit criteria:

- new vocabulary works immediately after insertion
- repeated user choices visibly shift rankings without retraining

#### 2.4 Train Offline Judge Baselines

Once the feature row is stable and memory exists, benchmark the simplest useful
judges.

Required baselines:

- linear online-friendly model
- small tree model as the strongest offline tabular baseline

Initial task:

- pointwise binary classification:
  - "is this candidate the right correction?"

Evaluation must include:

- time-aware splits that simulate user history
- term-family-isolated splits
- speaker/session isolation where applicable
- protected slices:
  - acronyms
  - mixed alphanumeric identifiers
  - snake_case and camelCase forms
  - person-name-like near neighbors
  - common-English-word vs technical-term collisions
  - very short spans

Random splits are acceptable as smoke tests only.

Exit criteria:

- a tiny judge materially beats deterministic-only selection
- memory features help on real held-out cases
- calibration is good enough for abstention work to be meaningful

#### 2.5 Decide the Next Leverage Point

After the first offline judge results, decide what actually matters next.

The likely forks are:

1. if shortlist quality is still the bottleneck:
   - improve retrieval next
   - likely with structure-aware retrieval upgrades, then articulatory-feature
     retrieval lanes
2. if shortlist quality is good but near-neighbor ranking is the bottleneck:
   - move to the tiny final judge
3. if tiny judge helps but remaining cases are still stubborn:
   - benchmark frozen neural fallback on ambiguous top candidates only

This decision should be evidence-based, not assumed in advance.

### Bucket 3: Deferred Productization

These things are probably right eventually, but they should not block de-risking
the core loop.

Do them after Bucket 2 proves the core plan is real:

- richer bundle manifest enforcement
- promotion policy automation
- shadow and suggestion modes
- online weight updates
- rollback checkpoints for learned weights
- neural fallback integration
- richer deployment and compatibility ceremony

These are productization tasks, not prerequisites for learning whether the core
design works.

### Evaluation Contract

The evaluation model itself is committed now, even if some scoreboards arrive
incrementally.

The core oracle layers are:

1. span proposal recall
   - did we even propose the right region?
2. shortlist recall at k given span
   - did retrieval surface the right term?
3. oracle judge accuracy given shortlist
   - if the right answer is present, what is the best possible selection
     accuracy?
4. end-to-end correction accuracy
   - what happened in the full pipeline?

For any failed eval case, the system should be able to say:

- what was the first failing stage?
- whether downstream stages ever had a chance to recover

These four layers must be operational together before we trust learned-judge
results.

### What We Should Not Do By Default

Do not make these the main plan:

- per-user LoRA or adapter fine-tuning
- storing new vocabulary in model weights
- generic prompted LLM judging as the default final decision maker
- building the full productization shell before the core loop is de-risked

Those can remain experiments or later infrastructure work. They should not be
the default execution path.

## Acceptance Criteria

This system is in the right shape when all of the following are true:

- the production correction path runs entirely from versioned artifacts
- retrieval and final judging are measurable separately
- a bad correction can be explained end-to-end in `beeml-web`
- a missed correction can be localized to:
  - alias coverage
  - retrieval
  - verification
  - final judging
- `beeml-web` is useful as both a product surface and a development debugger

That is the target to build toward.

## Appendix A: Phonetic Retrieval Roadmap

This appendix is the retrieval-specific technical roadmap that supports the
main system plan above.

The phase numbering in this appendix is local to retrieval work. It is not the
same as the main roadmap phase numbering above.

This appendix is subordinate to the main roadmap above. If the two ever appear
to disagree, the main roadmap wins.

### Goal

Replace the current brute-force span proposal path with an indexed phonetic
retrieval pipeline that:

- starts from a strong ASR transcript (`Qwen/MLX`)
- uses forced-aligner timings for locality
- uses `eSpeak` IPA as the main searchable representation
- retrieves plausible correction candidates quickly enough for interactive use
- defers context-dependent choice to the later final-judge stage

This appendix is specifically for the non-ZIPA path.

### Current Takeaway

What appears to be true now:

- `Qwen/MLX` transcript quality is already strong enough to be the main text
  source
- forced aligner timings are already available and good enough for region
  locality
- `eSpeak` IPA has produced the best candidate pairs so far
- the real unsolved problem is efficient candidate retrieval over phonetic
  forms
- the current expensive step is effectively `many transcript spans x many
  lexicon entries`

### Problem Statement

Given:

- a transcript with word timings
- optional confidence per token or word later
- `eSpeak` IPA for transcript words and spans
- a lexicon containing canonical terms, spoken variants, and confusion pairs

we need:

- a fast way to retrieve plausible correction candidates for contiguous
  transcript spans
- without brute-forcing every span against every lexicon entry
- while preserving enough provenance to debug retrieval failures and tune
  ranking

We also need retrieval evaluation to distinguish:

- span proposal failures
- shortlist failures given a correct span
- later judge failures on a good shortlist

### Non-Goals

For this appendix, do not optimize for:

- audio-based retrieval
- ZIPA-first alignment
- end-to-end neural spoken term detection
- whole-sentence correction as the primary retrieval mechanism

ZIPA can be reevaluated later as an auxiliary signal. It is not the center of
this design.

### Target Architecture

The pipeline should become:

1. `Qwen/MLX` transcript
2. forced-aligner word timings
3. `eSpeak` IPA projection for words and spans
4. phonetic retrieval index
5. shortlist verification with feature-aware phonetic distance
6. candidate feature export for the final judge
7. context-dependent judging over a small number of local candidates

The key separation is:

- **retrieval** decides which spans and term candidates are plausible
- **final judging** decides which candidate fits the sentence context, using
  deterministic scores, tiny learned scoring, and optional neural fallback

### Phase 1: Lexicon Expansion

Build a normalized lexicon representation for retrieval.

Each lexicon entry should include:

- `term`
- `alias_text`
- `alias_source`
  - `canonical`
  - `spoken`
  - `confusion`
- `ipa`
- `ipa_reduced`
- `token_count`
- `phone_count`
- `identifier_flags`
  - acronym-like
  - contains digits
  - snake/camel/symbol-derived

Do not add G2P N-best yet.

Reason:

- human-entered spoken variants and confusion pairs are already high-signal
- G2P expansions are likely to increase candidate noise early

### Phase 2: Primary Retrieval Index

Build one main index first:

- boundary-aware IPA 2-gram postings
- boundary-aware IPA 3-gram postings
- length bucket
- token-count bucket

This should be implemented as an inverted index, not brute-force scan.

Suggested stored retrieval features per alias:

- raw IPA q-grams
- reduced IPA q-grams
- start/end boundary grams
- token-count bucket
- phone-length bucket

The index should return a shortlist with provenance, not just term ids.

Each retrieved hit should retain:

- `term`
- `alias_source`
- `matched_alias`
- `which_index_view_matched`
- `qgram_overlap_count`
- `length_bucket_match`
- `token_count_match`

### Phase 3: Span Enumeration

We still need a span proposal strategy, but it should be cheap and explicit.

Initial version:

- enumerate contiguous spans up to 4 or 5 words
- derive span IPA from `eSpeak`
- query the phonetic index for each span

Keep span metadata:

- token range
- char range
- time range from forced aligner
- original text
- IPA
- word count

This is still potentially expensive, so it must be paired with retrieval
filters:

- length bucket match
- token-count compatibility
- q-gram overlap threshold

Span policy should be versioned and explicit.

At minimum, the retrieval layer must define:

- maximum span length
- punctuation and identifier-boundary handling
- how overlapping spans are retained for later region assembly
- what provenance is preserved so end-to-end failures can be attributed to span
  policy rather than retrieval

### Phase 4: Verification

Only verify the top shortlist per span.

Recommended verifier:

- feature-aware phonetic distance
- use `rspanphon` as the current base

Verifier output should include:

- normalized phonetic similarity
- raw feature distance
- candidate/source metadata
- exact/compact/prefix indicators if useful
- reusable candidate feature fields for later judging

This verifier should be authoritative for shortlist refinement, but not used as
the full search algorithm.

### Phase 5: Candidate Feature Export

Before a learned final judge is stable, the retrieval and verifier layers must
emit one stable and versioned candidate feature record.

This should include:

- retrieval features
- verification features
- structure features
- source priors
- context window features

This becomes the interface between phonetic retrieval and the later final
judge.

It should include enough provenance for oracle metrics:

- span-proposal identity
- retrieval rank
- shortlist membership
- verification survival

### Phase 6: Contextual Final Judge

The final judge should be local and comparative.

It should not score random full-sentence mutations independently.

For each proposed region:

- original sentence
- left context
- right context
- original span
- candidate replacements
- keep-original option

Ask the judge to choose among local alternatives for that region.

This stage should consume a small, already filtered set of candidates.

The default learned path should be:

- tiny feature-based scorer first
- frozen neural reranker only for ambiguous cases

### Phase 7: Short-Query Lane

Short phonetic strings behave differently and will likely need special
handling.

Do this only after the primary index works.

Options:

- stricter thresholds for very short IPA queries
- explicit acronym and identifier rules
- later: trie/TST + Levenshtein automaton or deletion index for `k=1`-style
  short queries

Do not build the full short-query sidecar before the main q-gram path is
stable.

### Debuggability Requirements

Every retrieval result must preserve provenance.

This is mandatory.

For each shortlisted candidate, we need to know:

- which span generated it
- which alias source produced it
- which index view retrieved it
- why it survived filtering
- verifier score
- whether the final judge accepted it

Without this, tuning will be guesswork.

This is also what makes oracle evaluation possible.

### Recommended Module Boundaries

Add new backend modules roughly along these lines:

- `phonetic_lexicon.rs`
  - lexicon expansion
  - alias normalization
  - IPA storage

- `phonetic_index.rs`
  - q-gram postings
  - retrieval query path
  - shortlist generation

- `phonetic_verify.rs`
  - feature-aware verification
  - score explanation/debug output

- `region_proposal.rs`
  - span enumeration
  - early filters
  - non-overlapping proposal selection later

Existing sentence-choice logic can initially stay where it is, but it should
consume the new retrieval outputs.

### Evaluation Plan

We need two retrieval-adjacent evaluation loops here.

#### 1. Retrieval Benchmark

Measure retrieval independent of final judging.

For a fixed set of human examples:

- was the target term retrieved at all?
- was it in top 1 / top 3 / top 10?
- how many spans were queried?
- how many candidates were verified?
- retrieval latency

#### 2. End-to-End Correction Eval

Measure:

- exact sentence recovery
- target term recovery
- target proposed
- target accepted
- latency breakdown

Failure buckets should remain explicit:

- no proposal
- target proposed but not selected
- wrong proposal selected
- target-only partial fix

### Implementation Order

Recommended execution order:

1. lexicon normalization for phonetic retrieval
2. q-gram inverted index over IPA
3. span query -> shortlist path
4. feature-aware verifier on shortlist
5. retrieval benchmark
6. candidate feature export
7. tiny learned judge on top of the shortlist
8. short-query special handling
9. optional later work:
   - feature q-gram view
   - G2P expansions
   - automaton/trie short-query sidecar
   - WFST-based alias/verbalization path

### Success Criteria

The first prototype is successful if:

- it eliminates brute-force span-vs-lexicon search
- target retrieval recall is materially better than current brute-force
  heuristics
- it is fast enough for interactive correction
- it is diagnosable when it fails

The first prototype does **not** need:

- perfect context handling
- G2P expansions
- every possible phonetic view
- ZIPA integration

It needs to produce a trustworthy, inspectable shortlist.

### Open Questions

- how aggressively should spans be enumerated before retrieval cost dominates?
- should reduced IPA be in the first index or added only after baseline
  results?
- when should articulatory-feature retrieval lanes be promoted from retrieval
  upgrade to baseline requirement?
- what is the best verifier threshold policy for acronym-like terms?
- can `Qwen/MLX` token confidence be surfaced soon enough to guide span
  proposal early?
