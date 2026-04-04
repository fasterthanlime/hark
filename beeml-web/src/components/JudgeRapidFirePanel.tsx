import { useCallback, useEffect, useMemo, useState } from "react";
import { connectBeeMl } from "../beeml.generated";
import type {
  RapidFireChoice,
  RetrievalPrototypeProbeResult,
  RetrievalPrototypeTeachingCase,
} from "../beeml.generated";
import { makeApproximateWords } from "./retrievalPrototypeUtils";


export function JudgeRapidFirePanel({
  wsUrl,
}: {
  wsUrl: string;
}) {
  const [deckLimit, setDeckLimit] = useState(80);
  const [maxSpanWords, setMaxSpanWords] = useState(4);
  const [shortlistLimit, setShortlistLimit] = useState(8);
  const [verifyLimit, setVerifyLimit] = useState(5);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cases, setCases] = useState<RetrievalPrototypeTeachingCase[]>([]);
  const [caseIndex, setCaseIndex] = useState(0);
  const [probeResult, setProbeResult] = useState<RetrievalPrototypeProbeResult | null>(null);
  const [teachingKey, setTeachingKey] = useState<string | null>(null);

  const currentCase = cases[caseIndex] ?? null;

  const loadDeck = useCallback(async () => {
    try {
      setStatus("Loading teaching deck...");
      setError(null);
      const client = await connectBeeMl(wsUrl);
      const result = await client.loadRetrievalPrototypeTeachingDeck({
        limit: deckLimit,
        include_counterexamples: true,
      });
      if (!result.ok) throw new Error(result.error);
      setCases(result.value.cases);
      setCaseIndex(0);
      setProbeResult(null);
      setStatus(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus(null);
    }
  }, [deckLimit, wsUrl]);

  const probeCurrentCase = useCallback(async () => {
    if (!currentCase) return;
    try {
      setStatus("Scoring...");
      setError(null);
      const client = await connectBeeMl(wsUrl);
      const result = await client.probeRetrievalPrototype({
        transcript: currentCase.transcript,
        words: makeApproximateWords(currentCase.transcript),
        max_span_words: maxSpanWords,
        shortlist_limit: shortlistLimit,
        verify_limit: verifyLimit,
        expected_source_text: currentCase.source_text,
      });
      if (!result.ok) throw new Error(result.error);
      setProbeResult(result.value);
      setStatus(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus(null);
    }
  }, [currentCase, maxSpanWords, shortlistLimit, verifyLimit, wsUrl]);

  useEffect(() => {
    if (cases.length === 0) return;
    void probeCurrentCase();
  }, [caseIndex, cases.length, probeCurrentCase]);

  const rapidFire = probeResult?.rapid_fire ?? null;

  const teach = useCallback(
    async (choice: RapidFireChoice) => {
      if (!currentCase) return;
      try {
        const key = `${currentCase.case_id}:${choice.option_id}`;
        setTeachingKey(key);
        setStatus("Teaching...");
        setError(null);
        const client = await connectBeeMl(wsUrl);
        const result = await client.teachRetrievalPrototypeJudge({
          probe: {
            transcript: currentCase.transcript,
            words: makeApproximateWords(currentCase.transcript),
            max_span_words: maxSpanWords,
            shortlist_limit: shortlistLimit,
            verify_limit: verifyLimit,
            expected_source_text: currentCase.source_text,
          },
          span_token_start: choice.span_token_start,
          span_token_end: choice.span_token_end,
          choose_keep_original: choice.choose_keep_original,
          chosen_alias_id: choice.choose_keep_original ? null : choice.chosen_alias_id,
          reject_group: false,
          rejected_group_spans: [],
        });
        if (!result.ok) throw new Error(result.error);
        setProbeResult(result.value);
        setCaseIndex((index) => Math.min(index + 1, Math.max(cases.length - 1, 0)));
        setStatus(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setStatus(null);
      } finally {
        setTeachingKey(null);
      }
    },
    [cases.length, currentCase, maxSpanWords, shortlistLimit, verifyLimit, wsUrl],
  );

  const rejectGroup = useCallback(async () => {
    if (!currentCase || !rapidFire) return;
    try {
      setTeachingKey(`${currentCase.case_id}:reject-group`);
      setStatus("Teaching...");
      setError(null);
      const client = await connectBeeMl(wsUrl);
      const result = await client.teachRetrievalPrototypeJudge({
        probe: {
          transcript: currentCase.transcript,
          words: makeApproximateWords(currentCase.transcript),
          max_span_words: maxSpanWords,
          shortlist_limit: shortlistLimit,
          verify_limit: verifyLimit,
          expected_source_text: currentCase.source_text,
        },
        span_token_start: 0,
        span_token_end: 0,
        choose_keep_original: true,
        chosen_alias_id: null,
        reject_group: true,
        rejected_group_spans: rapidFire.rejected_group_spans,
      });
      if (!result.ok) throw new Error(result.error);
      setProbeResult(result.value);
      setCaseIndex((index) => Math.min(index + 1, Math.max(cases.length - 1, 0)));
      setStatus(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus(null);
    } finally {
      setTeachingKey(null);
    }
  }, [cases.length, currentCase, maxSpanWords, rapidFire, shortlistLimit, verifyLimit, wsUrl]);

  // Pre-load: config form
  if (cases.length === 0) {
    return (
      <div className="prototype-lab prototype-stack">
        <section className="prototype-card">
          <header>
            <strong>Judge Rapid Fire</strong>
            <span>Load a deck, then pick the correct sentence for each case.</span>
          </header>
          <div className="control-bar">
            <div className="numeric-row">
              <label>
                <span>deck</span>
                <input type="number" min={1} max={500} value={deckLimit}
                  onChange={(e) => setDeckLimit(Number(e.target.value) || 1)} />
              </label>
              <label>
                <span>max span words</span>
                <input type="number" min={1} max={8} value={maxSpanWords}
                  onChange={(e) => setMaxSpanWords(Number(e.target.value) || 1)} />
              </label>
              <label>
                <span>shortlist</span>
                <input type="number" min={1} max={20} value={shortlistLimit}
                  onChange={(e) => setShortlistLimit(Number(e.target.value) || 1)} />
              </label>
              <label>
                <span>verify</span>
                <input type="number" min={1} max={20} value={verifyLimit}
                  onChange={(e) => setVerifyLimit(Number(e.target.value) || 1)} />
              </label>
            </div>
            <button className="primary" aria-label="Load Deck" onClick={() => void loadDeck()}>Load Deck</button>
          </div>
          {(status || error) && (
            <div className="notice-row">
              {status && <span className="status-pill">{status}</span>}
              {error && <span className="error-pill">{error}</span>}
            </div>
          )}
        </section>
      </div>
    );
  }

  // Loaded: rapid fire interface
  const expected = currentCase?.should_abstain ? "Keep original" : currentCase?.target_term;

  return (
    <div className="prototype-lab prototype-stack">
      {/* Case header bar */}
      <section className="prototype-card prototype-card-tight rapid-fire-toolbar">
        <div className="rapid-fire-header-inline" style={{ justifyContent: "space-between", width: "100%" }}>
          <div style={{ display: "flex", alignItems: "center", gap: "0.5rem" }}>
            <strong>Rapid Fire</strong>
            <span className="mini-badge">{caseIndex + 1} / {cases.length}</span>
            {status && <span className="status-pill">{status}</span>}
            {error && <span className="error-pill">{error}</span>}
          </div>
          <div className="control-actions control-actions-compact">
            <button className="compact-nav" aria-label="Previous case"
              onClick={() => setCaseIndex((i) => Math.max(i - 1, 0))}
              disabled={caseIndex === 0}>Prev</button>
            <button className="compact-nav" aria-label="Next case"
              onClick={() => setCaseIndex((i) => Math.min(i + 1, Math.max(cases.length - 1, 0)))}
              disabled={caseIndex >= cases.length - 1}>Next</button>
          </div>
        </div>
      </section>

      {currentCase && (
          <div className="choice-list">
            {/* Row: expected correct sentence (gold) */}
            <div className="choice-row choice-row-context choice-row-expected">
              <div className="choice-row-main">
                <div className="sentence-preview-line">{currentCase.source_text}</div>
              </div>
              <div className="choice-row-meta">
                <span className="choice-flags">expected</span>
                <span className="badge">{currentCase.suite}</span>
              </div>
            </div>

            {/* Row: transcript (clickable — "keep as-is" / reject all corrections) */}
            <button className="choice-button choice-button-keep"
              aria-label="Keep transcript"
              disabled={teachingKey === `${currentCase.case_id}:reject-group`}
              onClick={() => void rejectGroup()}>
              <div className="choice-row-main">
                <div className="sentence-preview-line">{currentCase.transcript}</div>
              </div>
              <div className="choice-row-meta">
                <span className="choice-flags">transcript</span>
              </div>
            </button>

            {rapidFire ? (
              <>
                {rapidFire.choices.map((choice) => {
                  const key = `${currentCase.case_id}:${choice.option_id}`;
                  const classes = ["choice-button"];
                  if (choice.choose_keep_original) classes.push("choice-button-keep");
                  if (choice.is_judge_pick) classes.push("choice-button-current");
                  if (choice.is_gold) classes.push("choice-button-gold");
                  return (
                    <button key={key} className={classes.join(" ")}
                      disabled={teachingKey === key}
                      onClick={() => void teach(choice)}>
                      <div className="choice-row-main">
                        <div className="sentence-preview-line">{choice.sentence}</div>
                      </div>
                      <div className="choice-row-meta">
                        <span className="choice-flags">
                          {choice.is_judge_pick ? "judge" : ""}
                          {choice.is_judge_pick && choice.is_gold ? " · " : ""}
                          {choice.is_gold ? "gold" : ""}
                          {!choice.choose_keep_original ? ` · ${choice.replaced_text} -> ${choice.replacement_text}` : " · keep original"}
                        </span>
                        <span className="choice-score">{choice.probability.toFixed(3)}</span>
                      </div>
                    </button>
                  );
                })}
              </>
            ) : (
                <div className="choice-row choice-row-context">
                  <div className="choice-row-main">
                  <span style={{ color: "var(--text-muted)" }}>No usable decision set for this case.</span>
                  </div>
                </div>
            )}
          </div>
      )}
    </div>
  );
}
