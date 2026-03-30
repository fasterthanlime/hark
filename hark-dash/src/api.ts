import type {
  EvalInspectorData,
  BakeoffResult,
  BakeoffEntry,
  Job,
  TimedToken,
} from "./types";

// --- Raw backend shapes (private, normalized immediately) ---

type RawAsrDualResponse = {
  parakeet: string;
  parakeet_alignment: TimedToken[];
  correction_input: string;
  // legacy fields we ignore
  qwen?: string;
  cohere?: string;
};

type RawCorrectResponse = {
  original: string;
  corrected: string;
  accepted: unknown[];
  proposals: unknown[];
  sentence_candidates: unknown[];
  alignments: {
    timing_source: string;
    espeak?: TimedToken[];
    qwen?: TimedToken[];
    zipa?: TimedToken[];
    zipa_espeak?: TimedToken[];
    zipa_qwen?: TimedToken[];
    expected?: TimedToken[];
    current?: TimedToken[];
    prototype?: TimedToken[];
  };
  zipa_trace?: unknown;
  reranker?: unknown;
};

type RawDetailResponse = {
  ok: boolean;
  recording_id: number;
  transcript_label?: string;
  transcript?: string;
  current?: string;
  qwen?: string;
  parakeet?: string;
  parakeet_alignment: TimedToken[];
  correction_input: string;
  elapsed_ms?: number;
  alignments: RawCorrectResponse["alignments"];
  zipa_trace?: unknown;
  prototype_trace?: {
    corrected: string;
    accepted: unknown[];
    proposals: unknown[];
    sentence_candidates: unknown[];
    reranker?: unknown;
  };
};

type RawBakeoffEntry = {
  term: string;
  case_id: string;
  source: string;
  expected: string;
  qwen: string;
  recording_id: number;
  hit_count: number;
  prototype_ok: boolean;
  prototype_target_ok: boolean;
  prototype: string;
  analysis: {
    failure_reason: string;
    exact_ok: boolean;
    target_ok: boolean;
  };
  prototype_trace_excerpt: {
    proposal_count: number;
    sentence_candidate_count: number;
    accepted_count: number;
  };
};

type RawBakeoffResult = {
  source: string;
  limit: number;
  processed: number;
  summary: {
    n: number;
    prototype: number;
    prototype_wrong: number;
    both_wrong: number;
  };
  entries: RawBakeoffEntry[];
};

// --- Normalization ---

function normalizeAlignments(raw: RawCorrectResponse["alignments"]) {
  return {
    timingSource: raw.timing_source,
    expected: raw.expected,
    espeak: raw.espeak ?? raw.qwen ?? [],
    current: raw.current,
    prototype: raw.prototype,
    zipa: raw.zipa,
    zipaEspeak: raw.zipa_espeak ?? raw.zipa_qwen ?? [],
  };
}

function normalizeCorrectResponse(
  raw: RawCorrectResponse,
  transcript: string,
  transcriptLabel: string,
  parakeetAlignment: TimedToken[],
  correctionInput: string,
): EvalInspectorData {
  return {
    transcript,
    transcriptLabel,
    transcriptSource: "parakeet",
    parakeetAlignment,
    correctionInput,
    alignments: normalizeAlignments(raw.alignments),
    zipaTrace: raw.zipa_trace,
    prototype: {
      corrected: raw.corrected,
      accepted: raw.accepted as EvalInspectorData["prototype"]["accepted"],
      proposals: raw.proposals as EvalInspectorData["prototype"]["proposals"],
      sentenceCandidates: raw.sentence_candidates,
      reranker: raw.reranker,
    },
  };
}

function normalizeDetailResponse(raw: RawDetailResponse): EvalInspectorData {
  const transcript =
    raw.transcript ?? raw.current ?? raw.parakeet ?? raw.qwen ?? "";
  const transcriptLabel = raw.transcript_label ?? "Parakeet";
  const pt = raw.prototype_trace;
  return {
    transcript,
    transcriptLabel,
    transcriptSource: "parakeet",
    parakeetAlignment: raw.parakeet_alignment ?? [],
    correctionInput: raw.correction_input,
    elapsedMs: raw.elapsed_ms,
    alignments: normalizeAlignments(raw.alignments),
    zipaTrace: raw.zipa_trace,
    prototype: pt
      ? {
          corrected: pt.corrected,
          accepted: pt.accepted as EvalInspectorData["prototype"]["accepted"],
          proposals: pt.proposals as EvalInspectorData["prototype"]["proposals"],
          sentenceCandidates: pt.sentence_candidates,
          reranker: pt.reranker,
        }
      : { corrected: "", accepted: [], proposals: [], sentenceCandidates: [] },
  };
}

function normalizeBakeoffEntry(raw: RawBakeoffEntry): BakeoffEntry {
  return {
    term: raw.term,
    caseId: raw.case_id,
    source: raw.source,
    expected: raw.expected,
    transcript: raw.qwen, // legacy field → canonical
    recordingId: raw.recording_id,
    hitCount: raw.hit_count,
    prototypeOk: raw.prototype_ok,
    prototypeTargetOk: raw.prototype_target_ok,
    prototype: raw.prototype,
    analysis: {
      failureReason: raw.analysis.failure_reason,
      exactOk: raw.analysis.exact_ok,
      targetOk: raw.analysis.target_ok,
    },
    prototypeTraceExcerpt: {
      proposalCount: raw.prototype_trace_excerpt.proposal_count,
      sentenceCandidateCount:
        raw.prototype_trace_excerpt.sentence_candidate_count,
      acceptedCount: raw.prototype_trace_excerpt.accepted_count,
    },
  };
}

function normalizeBakeoffResult(raw: RawBakeoffResult): BakeoffResult {
  return {
    source: raw.source,
    limit: raw.limit,
    processed: raw.processed,
    summary: {
      n: raw.summary.n,
      prototype: raw.summary.prototype,
      prototypeWrong: raw.summary.prototype_wrong,
      bothWrong: raw.summary.both_wrong,
    },
    entries: raw.entries.map(normalizeBakeoffEntry),
  };
}

// --- API calls ---

async function post<T>(url: string, body: unknown): Promise<T> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

/** Record audio → ASR transcription */
export async function asrDual(audioWav: Blob): Promise<RawAsrDualResponse> {
  const res = await fetch("/api/asr/dual", {
    method: "POST",
    headers: { "Content-Type": "audio/wav" },
    body: audioWav,
  });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

/** Run correction on a transcript */
export async function correctPrototype(params: {
  transcript: string;
  audioWavBase64?: string;
  trainId: number;
}): Promise<EvalInspectorData> {
  const raw = await post<RawCorrectResponse>("/api/correct-prototype", {
    transcript: params.transcript,
    audio_wav_base64: params.audioWavBase64,
    use_model_reranker: true,
    use_prototype_adapters: true,
    reranker_mode: "trained",
    prototype_reranker_train_id: params.trainId,
  });
  return normalizeCorrectResponse(raw, params.transcript, "Parakeet", [], "parakeet");
}

/** Start a human eval bakeoff job */
export async function startBakeoff(params: {
  limit: number;
  trainId: number;
  randomize?: boolean;
  sampleSeed?: number;
}): Promise<{ jobId: number }> {
  const raw = await post<{ job_id: number }>(
    "/api/correct-prototype/bakeoff",
    {
      source: "human",
      limit: params.limit,
      randomize: params.randomize ?? true,
      sample_seed: params.sampleSeed,
      use_model_reranker: true,
      use_prototype_adapters: true,
      reranker_mode: "trained",
      prototype_reranker_train_id: params.trainId,
    },
  );
  return { jobId: raw.job_id };
}

/** Poll a job's status */
export async function getJob(id: number): Promise<Job> {
  const res = await fetch(`/api/jobs/${id}`);
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  const raw = await res.json();
  return {
    id: raw.id,
    jobType: raw.job_type,
    status: raw.status,
    config: raw.config,
    log: raw.log,
    result: raw.result,
    createdAt: raw.created_at,
    finishedAt: raw.finished_at,
  };
}

/** Parse a completed bakeoff job's result */
export function parseBakeoffResult(job: Job): BakeoffResult | null {
  if (!job.result) return null;
  const raw: RawBakeoffResult = JSON.parse(job.result);
  return normalizeBakeoffResult(raw);
}

/** Lazy-load full detail for one human eval case */
export async function bakeoffDetail(params: {
  recordingId: number;
  transcript: string;
  expected: string;
  prototype: string;
  trainId: number;
}): Promise<EvalInspectorData> {
  const raw = await post<RawDetailResponse>(
    "/api/correct-prototype/bakeoff/detail",
    {
      source: "human",
      recording_id: params.recordingId,
      transcript: params.transcript,
      expected: params.expected,
      current: params.transcript,
      prototype: params.prototype,
      use_model_reranker: true,
      use_prototype_adapters: true,
      reranker_mode: "trained",
      prototype_reranker_train_id: params.trainId,
    },
  );
  return normalizeDetailResponse(raw);
}

/** Audio URL for a human eval recording */
export function recordingAudioUrl(recordingId: number): string {
  return `/api/author/recordings/${recordingId}/audio`;
}
