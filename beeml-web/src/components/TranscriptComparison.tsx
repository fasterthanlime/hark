import type { AcceptedEdit } from "../types";

function DiffLine({ label, text, color }: { label: string; text: string; color?: string }) {
  return (
    <div className="diff-line">
      <span className="label" style={color ? { color } : undefined}>{label}</span>
      <span className="text">{text || "\u2014"}</span>
    </div>
  );
}

export function TranscriptComparison({
  transcriptLabel,
  transcript,
  expected,
  corrected,
  accepted,
}: {
  transcriptLabel: string;
  transcript: string;
  expected?: string;
  corrected: string;
  accepted: AcceptedEdit[];
}) {
  return (
    <div>
      <DiffLine label={transcriptLabel} text={transcript} />
      {expected && <DiffLine label="Expected" text={expected} />}
      <DiffLine label="Reranker" text={corrected} color="var(--warning)" />

      {accepted.length > 0 && (
        <div style={{ marginTop: "1rem" }}>
          <div className="accepted-edits-heading">ACCEPTED EDITS</div>
          <div className="accepted-edits">
            {accepted.map((edit, i) => (
              <span key={i} className="accepted-edit">
                <span className="original">{edit.original}</span>
                <span>&rarr;</span>
                <span className="replacement">{edit.replacement}</span>
                {edit.score != null && (
                  <span className="score">{edit.score.toFixed(2)}</span>
                )}
                {edit.delta != null && (
                  <span className={`delta ${edit.delta < 0 ? "negative" : "positive"}`}>
                    &Delta; {edit.delta > 0 ? "+" : ""}{edit.delta.toFixed(2)}
                  </span>
                )}
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
