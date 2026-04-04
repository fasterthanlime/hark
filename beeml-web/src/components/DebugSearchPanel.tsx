import { memo, useState } from "react";
import type {
  RetrievalPrototypeProbeResult,
  SpanDebugTrace,
} from "../beeml.generated";

export const DebugSearchPanel = memo(function DebugSearchPanel({
  probeResult,
  rapidFire,
  targetTerm,
}: {
  probeResult: RetrievalPrototypeProbeResult;
  rapidFire: NonNullable<RetrievalPrototypeProbeResult["rapid_fire"]>;
  targetTerm: string;
}) {
  const [open, setOpen] = useState(false);

  return (
    <details className="debug-search-expander" onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
      <summary>
        Debug search
        {rapidFire.no_exact_match && <span className="debug-warn" style={{ marginLeft: "0.5rem" }}>no exact match</span>}
        <span style={{ marginLeft: "0.5rem", opacity: 0.5 }}>
          {rapidFire.search_mode} · {probeResult.spans.length} spans
        </span>
      </summary>
      {open && <DebugSearchContent probeResult={probeResult} rapidFire={rapidFire} targetTerm={targetTerm} />}
    </details>
  );
});

const DebugSearchContent = memo(function DebugSearchContent({
  probeResult,
  rapidFire,
  targetTerm,
}: {
  probeResult: RetrievalPrototypeProbeResult;
  rapidFire: NonNullable<RetrievalPrototypeProbeResult["rapid_fire"]>;
  targetTerm: string;
}) {
  const interestingSpans = probeResult.spans
    .filter((s: SpanDebugTrace) => s.candidates.length > 0)
    .sort((a: SpanDebugTrace, b: SpanDebugTrace) => {
      const aHasTarget = a.candidates.some(c => c.term.toLowerCase() === targetTerm) ? 1 : 0;
      const bHasTarget = b.candidates.some(c => c.term.toLowerCase() === targetTerm) ? 1 : 0;
      if (aHasTarget !== bHasTarget) return bHasTarget - aHasTarget;
      const aMax = Math.max(...a.candidates.map(c => c.features.acceptance_score));
      const bMax = Math.max(...b.candidates.map(c => c.features.acceptance_score));
      return bMax - aMax;
    })
    .slice(0, 16);

  return (
    <div className="debug-search-content" style={{ fontSize: "0.95rem" }}>
      {/* Component composition */}
      {rapidFire.components.length > 0 && (
        <div style={{ marginBottom: "0.75rem", borderBottom: "1px solid var(--border, #333)", paddingBottom: "0.5rem" }}>
          <div style={{ fontWeight: 600, marginBottom: "0.25rem", fontSize: "0.85rem" }}>
            Components ({rapidFire.components.length}) · {rapidFire.total_combinations} combinations · {rapidFire.search_mode}
          </div>
          {rapidFire.components.map((comp) => {
            const spanRange = comp.spans.map(s => `${s.token_start}:${s.token_end}`).join(", ");
            return (
              <div key={comp.component_id} style={{ marginBottom: "0.5rem", borderLeft: "2px solid var(--border, #555)", paddingLeft: "0.5rem" }}>
                <div style={{ fontSize: "0.8rem", fontWeight: 600 }}>
                  component {comp.component_id} · tokens {spanRange}
                </div>
                {comp.hypotheses.map((hyp, hi) => (
                  <div key={hi} style={{ fontSize: "0.8rem", marginLeft: "0.5rem", display: "flex", justifyContent: "space-between", gap: "1rem" }}>
                    {hyp.choose_keep_original ? (
                      <span style={{ opacity: 0.5 }}>keep original</span>
                    ) : (
                      <span>
                        <span style={{ textDecoration: "line-through", opacity: 0.5 }}>{hyp.replaced_text}</span>
                        {" → "}
                        <span style={{ color: "var(--green, #22c55e)" }}>{hyp.replacement_text}</span>
                      </span>
                    )}
                    <span style={{ opacity: 0.6, fontVariantNumeric: "tabular-nums" }}>{hyp.probability.toFixed(3)}</span>
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      )}

      {/* Span-level retrieval diagnostics */}
      {interestingSpans.map((s: SpanDebugTrace, si: number) => {
        const hasTarget = s.candidates.some(c => c.term.toLowerCase() === targetTerm);
        return (
          <div key={si} style={{ marginBottom: "0.75rem", borderLeft: hasTarget ? "2px solid var(--green, #22c55e)" : "2px solid var(--border, #333)", paddingLeft: "0.5rem" }}>
            <div style={{ fontWeight: 600, marginBottom: "0.15rem" }}>
              "{s.span.text}" <span style={{ opacity: 0.5, fontWeight: 400 }}>tokens {s.span.token_start}:{s.span.token_end}</span>
            </div>
            <div style={{ fontSize: "0.8rem", opacity: 0.6, marginBottom: "0.25rem", fontFamily: "'Manuale IPA', serif" }}>
              ipa: {s.span.ipa_tokens.join(" ")}
            </div>
            {s.candidates.slice(0, 4).map((c, ci) => {
              const isTarget = c.term.toLowerCase() === targetTerm;
              const failedFilters = c.filter_decisions.filter(f => !f.passed);
              return (
                <div key={ci} style={{
                  fontSize: "0.8rem", marginLeft: "0.5rem", marginBottom: "0.15rem",
                  color: isTarget ? "var(--green, #22c55e)" : undefined,
                  opacity: c.accepted ? 1 : 0.6,
                }}>
                  <span style={{ fontWeight: isTarget ? 700 : 400 }}>{c.term}</span>
                  <span style={{ opacity: 0.5 }}> ({c.alias_source.tag})</span>
                  {" "}accept={c.features.acceptance_score.toFixed(2)}
                  {" "}phonetic={c.features.phonetic_score.toFixed(2)}
                  {" "}coarse={c.features.coarse_score.toFixed(2)}
                  {c.accepted ? " \u2713" : ""}
                  {failedFilters.length > 0 && (
                    <span style={{ color: "var(--red, #ef4444)" }}>
                      {" "}{failedFilters.map(f => `${f.name}: ${f.detail}`).join("; ")}
                    </span>
                  )}
                  <div style={{ fontFamily: "'Manuale IPA', serif", opacity: 0.5, marginLeft: "1rem" }}>
                    ipa: {c.alias_ipa_tokens.join(" ")}
                  </div>
                </div>
              );
            })}
            {s.candidates.length > 4 && (
              <div style={{ fontSize: "0.75rem", opacity: 0.4, marginLeft: "0.5rem" }}>
                +{s.candidates.length - 4} more candidates
              </div>
            )}
          </div>
        );
      })}
      {interestingSpans.length === 0 && (
        <div style={{ opacity: 0.5 }}>No spans produced any candidates.</div>
      )}
    </div>
  );
});
