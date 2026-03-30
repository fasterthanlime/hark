/** A timed token from alignment data */
export type TimedToken = {
  w: string;
  s: number;
  e: number;
  c?: number | null;
};

export type EvalTranscriptSource = "parakeet" | "transcript" | "unknown";

export type PrototypeAlignments = {
  timingSource: string;
  expected?: TimedToken[];
  espeak?: TimedToken[];
  current?: TimedToken[];
  prototype?: TimedToken[];
  zipa?: TimedToken[];
  zipaEspeak?: TimedToken[];
};

export type ProposalCandidate = {
  term: string;
  source: string;
  matchedForm?: string;
  rank: number;
  ipa: number;
  delta: number;
  phonemes?: string;
};

export type Proposal = {
  spanText: string;
  spanStart: number;
  spanEnd: number;
  candidates: ProposalCandidate[];
};

export type AcceptedEdit = {
  original: string;
  replacement: string;
  score: number;
  delta: number;
};

export type PrototypeTrace = {
  corrected: string;
  accepted: AcceptedEdit[];
  proposals: Proposal[];
  sentenceCandidates: unknown[];
  reranker?: unknown;
};

/** Canonical inspector data — normalized from any backend response */
export type EvalInspectorData = {
  transcript: string;
  transcriptLabel: string;
  transcriptError?: string | null;
  transcriptSource: EvalTranscriptSource;
  expected?: string;
  parakeetAlignment: TimedToken[];
  correctionInput: string;
  elapsedMs?: number | null;
  alignments: PrototypeAlignments;
  zipaTrace?: unknown;
  prototype: PrototypeTrace;
};

/** A single case from a human eval bakeoff run */
export type BakeoffEntry = {
  term: string;
  caseId: string;
  source: string;
  expected: string;
  transcript: string;
  recordingId: number;
  hitCount: number;
  prototypeOk: boolean;
  prototypeTargetOk: boolean;
  prototype: string;
  analysis: {
    failureReason: string;
    exactOk: boolean;
    targetOk: boolean;
  };
  prototypeTraceExcerpt: {
    proposalCount: number;
    sentenceCandidateCount: number;
    acceptedCount: number;
  };
};

export type BakeoffSummary = {
  n: number;
  prototype: number;
  prototypeWrong: number;
  bothWrong: number;
};

export type BakeoffResult = {
  source: string;
  limit: number;
  processed: number;
  summary: BakeoffSummary;
  entries: BakeoffEntry[];
};

export type JobStatus = "running" | "completed" | "failed";

export type Job = {
  id: number;
  jobType: string;
  status: JobStatus;
  config?: string | null;
  log: string;
  result?: string | null;
  createdAt: string;
  finishedAt?: string | null;
};
