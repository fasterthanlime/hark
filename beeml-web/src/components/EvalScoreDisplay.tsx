import { memo, useEffect, useState } from "react";
import { channel } from "@bearcove/vox-core";
import { connectBeeMl } from "../beeml.generated";
import type {
  RetrievalPrototypeEvalProgress,
  RetrievalPrototypeEvalResult,
} from "../beeml.generated";

/** Self-contained eval score display. Runs eval on mount and when triggerCount changes. */
export const EvalScoreDisplay = memo(function EvalScoreDisplay({
  wsUrl,
  maxSpanWords,
  triggerCount,
}: {
  wsUrl: string;
  maxSpanWords: number;
  triggerCount: number;
}) {
  const [evalResult, setEvalResult] = useState<RetrievalPrototypeEvalResult | null>(null);
  const [prevEvalResult, setPrevEvalResult] = useState<RetrievalPrototypeEvalResult | null>(null);
  const [evalProgress, setEvalProgress] = useState<RetrievalPrototypeEvalProgress | null>(null);
  const [evalRunning, setEvalRunning] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        setEvalRunning(true);
        setEvalProgress(null);
        const client = await connectBeeMl(wsUrl);
        const [progressTx, progressRx] = channel<RetrievalPrototypeEvalProgress>();

        const rpcPromise = client.runRetrievalPrototypeEval({
          limit: 500,
          max_span_words: maxSpanWords,
          shortlist_limit: 20,
          verify_limit: 20,
        }, progressTx).catch((e: unknown) => {
          console.error("eval RPC error:", e);
          return { ok: false as const, error: String(e) };
        });

        try {
          while (true) {
            const val = await progressRx.recv();
            if (val === null || cancelled) break;
            setEvalProgress(val);
          }
        } catch (e) {
          console.error("eval progress recv error:", e);
        }

        const response = await rpcPromise;
        if (cancelled) return;
        if (!response.ok) {
          console.error("eval failed:", response.error);
          return;
        }
        setEvalResult((prev) => {
          setPrevEvalResult(prev);
          return response.value;
        });
      } finally {
        if (!cancelled) {
          setEvalRunning(false);
          setEvalProgress(null);
        }
      }
    })();
    return () => { cancelled = true; };
  }, [wsUrl, maxSpanWords, triggerCount]);

  if (evalRunning && evalProgress) {
    return (
      <span style={{ fontVariantNumeric: "tabular-nums", fontSize: "1.1rem", fontWeight: 700, opacity: 0.7 }}>
        {evalProgress.judge_correct}/{evalProgress.evaluated}{" "}
        <span style={{ opacity: 0.4, fontSize: "0.85rem" }}>({evalProgress.total})</span>
      </span>
    );
  }

  if (evalRunning) {
    return <span className="status-pill">eval...</span>;
  }

  if (!evalResult) return null;

  const pct = Math.round((evalResult.judge_correct / evalResult.evaluated_cases) * 100);
  const delta = prevEvalResult
    ? evalResult.judge_correct - prevEvalResult.judge_correct
    : null;

  return (
    <span style={{ fontVariantNumeric: "tabular-nums", fontSize: "1.1rem", fontWeight: 700, letterSpacing: "-0.01em" }}>
      {evalResult.judge_correct}/{evalResult.evaluated_cases} ({pct}%)
      {delta !== null && delta !== 0 && (
        <span style={{ color: delta > 0 ? "var(--green, #22c55e)" : "var(--red, #ef4444)", marginLeft: "0.25rem" }}>
          {delta > 0 ? `+${delta}` : delta}
        </span>
      )}
    </span>
  );
});
