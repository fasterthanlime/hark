import { useRef, useState, useEffect, useCallback } from "react";
import type { PrototypeAlignments, TimedToken, SentenceCandidate, Reranker } from "../types";

type LaneToken = TimedToken & {
  dim?: boolean;
  editFrom?: string;
  editFromPhonemes?: string | null;
  editToPhonemes?: string | null;
  editVia?: string;
  editRank?: number;
  editSimilarity?: number | null;
};

type Lane = {
  label: string;
  tokens: LaneToken[];
  color: string;
  bg: string;
};

function applyCandidateEdits(
  transcriptTokens: TimedToken[],
  edits: SentenceCandidate["edits"],
): LaneToken[] {
  if (!transcriptTokens?.length) return [];

  const editByStart = new Map<number, (typeof edits)[0]>();
  for (const edit of edits) {
    editByStart.set(edit.tokenStart, edit);
  }

  const tokens: LaneToken[] = [];
  let i = 0;
  while (i < transcriptTokens.length) {
    const edit = editByStart.get(i);
    if (edit) {
      const startToken = transcriptTokens[edit.tokenStart];
      const endIdx = Math.min(edit.tokenEnd - 1, transcriptTokens.length - 1);
      const endToken = transcriptTokens[endIdx];
      if (startToken && endToken) {
        tokens.push({
          w: edit.to,
          s: startToken.s,
          e: endToken.e,
          dim: false,
          editFrom: edit.from,
          editFromPhonemes: edit.fromPhonemes,
          editToPhonemes: edit.toPhonemes,
          editVia: edit.via,
          editRank: edit.score,
          editSimilarity: edit.phoneticScore,
        });
      }
      i = edit.tokenEnd;
    } else {
      tokens.push({ ...transcriptTokens[i], dim: true });
      i++;
    }
  }
  return tokens;
}

function buildLanes(
  alignments: PrototypeAlignments,
  qwenAlignment: TimedToken[],
  sentenceCandidates?: SentenceCandidate[],
  reranker?: Reranker | null,
): Lane[] {
  const lanes: Lane[] = [];
  const transcriptBase = alignments.transcript ?? alignments.espeak ?? [];
  const candidates = sentenceCandidates ?? [];
  const chosenIdx = reranker?.chosenIndex;

  if (candidates.length > 0 && transcriptBase.length > 0) {
    const rerankerCandidates = reranker?.candidates;
    const indices = candidates.map((_, i) => i);
    indices.sort((a, b) => {
      if (a === chosenIdx) return -1;
      if (b === chosenIdx) return 1;
      const aProb = rerankerCandidates?.[a]?.yesProb ?? 0;
      const bProb = rerankerCandidates?.[b]?.yesProb ?? 0;
      return bProb - aProb;
    });

    for (const idx of indices) {
      const candidate = candidates[idx];
      const rc = rerankerCandidates?.[idx];
      const isChosen = idx === chosenIdx;
      const tokens = applyCandidateEdits(transcriptBase, candidate.edits);
      if (tokens.length === 0) continue;

      const pct = rc ? `${(rc.yesProb * 100).toFixed(0)}%` : "";
      const label = isChosen ? `\u2713 ${pct}` : `#${idx} ${pct}`;

      lanes.push({
        label,
        tokens,
        color: isChosen ? "var(--lane-reranker)" : "var(--text-dim)",
        bg: isChosen ? "var(--lane-reranker-bg)" : "transparent",
      });
    }
  }

  if (qwenAlignment.length > 0) {
    lanes.push({ label: "QWEN", tokens: qwenAlignment, color: "var(--lane-qwen)", bg: "var(--lane-qwen-bg)" });
  }
  if (alignments.espeak?.length) {
    lanes.push({ label: "eSpeak", tokens: alignments.espeak, color: "var(--lane-espeak)", bg: "var(--lane-espeak-bg)" });
  }
  if (alignments.zipaEspeak?.length) {
    lanes.push({ label: "ZIPA@eSpeak", tokens: alignments.zipaEspeak, color: "var(--lane-zipa-espeak)", bg: "var(--lane-zipa-espeak-bg)" });
  }
  if (alignments.zipa?.length) {
    lanes.push({ label: "ZIPA", tokens: alignments.zipa, color: "var(--lane-zipa)", bg: "var(--lane-zipa-bg)" });
  }

  return lanes;
}

const LABEL_WIDTH = 90;
const LANE_HEIGHT = 36;
const RULER_HEIGHT = 24;

type Selection = { laneIdx: number; tokenIdx: number } | null;

export function EvalTimeline({
  alignments,
  qwenAlignment,
  sentenceCandidates,
  reranker,
  currentTime,
  duration,
  onSeek,
  onPlayRange,
  zoom = 1,
}: {
  alignments: PrototypeAlignments;
  qwenAlignment: TimedToken[];
  sentenceCandidates?: SentenceCandidate[];
  reranker?: Reranker | null;
  currentTime: number;
  duration: number;
  onSeek: (time: number) => void;
  onPlayRange?: (start: number, end: number) => void;
  zoom?: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const lanes = buildLanes(alignments, qwenAlignment, sentenceCandidates, reranker);
  const lanesRef = useRef(lanes);
  lanesRef.current = lanes;
  const pxPerSec = 120 * zoom;

  const [selection, setSelection] = useState<Selection>(null);
  const [hover, setHover] = useState<{ laneIdx: number; tokenIdx: number; x: number; y: number } | null>(null);

  let maxEnd = duration;
  for (const lane of lanes) {
    for (const t of lane.tokens) {
      if (t.e > maxEnd) maxEnd = t.e;
    }
  }
  const totalWidth = Math.max(maxEnd, 0.01) * pxPerSec;

  const rulerRef = useRef<HTMLDivElement>(null);
  const seekFromX = useCallback(
    (clientX: number) => {
      const el = rulerRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const x = clientX - rect.left;
      onSeek(Math.max(0, Math.min(x / pxPerSec, duration)));
    },
    [pxPerSec, duration, onSeek],
  );

  const handleRulerPointerDown = useCallback(
    (e: React.PointerEvent) => {
      e.currentTarget.setPointerCapture(e.pointerId);
      setSelection(null);
      seekFromX(e.clientX);
    },
    [seekFromX],
  );

  const handleRulerPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (e.buttons === 0) return;
      seekFromX(e.clientX);
    },
    [seekFromX],
  );

  const selectAndPlay = useCallback(
    (laneIdx: number, tokenIdx: number) => {
      const lanes = lanesRef.current;
      if (laneIdx < 0 || laneIdx >= lanes.length) return;
      const tokens = lanes[laneIdx].tokens;
      if (tokenIdx < 0 || tokenIdx >= tokens.length) return;
      const token = tokens[tokenIdx];
      setSelection({ laneIdx, tokenIdx });
      if (onPlayRange) {
        onPlayRange(token.s, token.e);
      } else {
        onSeek(token.s);
      }
    },
    [onPlayRange, onSeek],
  );

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
      if (!selection) return;

      const lanes = lanesRef.current;
      const { laneIdx, tokenIdx } = selection;

      if (e.code === "ArrowLeft") {
        e.preventDefault();
        e.stopPropagation();
        selectAndPlay(laneIdx, tokenIdx - 1);
      } else if (e.code === "ArrowRight") {
        e.preventDefault();
        e.stopPropagation();
        selectAndPlay(laneIdx, tokenIdx + 1);
      } else if (e.code === "ArrowUp") {
        e.preventDefault();
        if (laneIdx > 0) {
          const curToken = lanes[laneIdx].tokens[tokenIdx];
          const closest = findClosestToken(lanes[laneIdx - 1].tokens, curToken.s);
          selectAndPlay(laneIdx - 1, closest);
        }
      } else if (e.code === "ArrowDown") {
        e.preventDefault();
        if (laneIdx < lanes.length - 1) {
          const curToken = lanes[laneIdx].tokens[tokenIdx];
          const closest = findClosestToken(lanes[laneIdx + 1].tokens, curToken.s);
          selectAndPlay(laneIdx + 1, closest);
        }
      } else if (e.code === "Escape") {
        setSelection(null);
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [selection, selectAndPlay]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const playheadX = LABEL_WIDTH + currentTime * pxPerSec;
    const viewWidth = el.clientWidth;
    const relX = playheadX - el.scrollLeft;
    if (relX > viewWidth * 0.75 || relX < LABEL_WIDTH) {
      el.scrollLeft = playheadX - viewWidth * 0.25;
    }
  }, [currentTime, pxPerSec]);

  if (lanes.length === 0) {
    return <div className="timeline-empty">No alignment data available</div>;
  }

  const playheadX = LABEL_WIDTH + currentTime * pxPerSec;
  const contentHeight = RULER_HEIGHT + lanes.length * LANE_HEIGHT;

  const rulerTicks: number[] = [];
  let tickInterval = 1;
  if (pxPerSec < 60) tickInterval = 2;
  if (pxPerSec < 30) tickInterval = 5;
  if (pxPerSec > 200) tickInterval = 0.5;
  if (pxPerSec > 500) tickInterval = 0.25;
  for (let t = 0; t <= maxEnd + tickInterval; t += tickInterval) {
    rulerTicks.push(t);
  }

  return (
    <div ref={containerRef} className="timeline hide-scrollbar">
      <div className="timeline-inner" style={{ width: LABEL_WIDTH + totalWidth, minHeight: contentHeight }}>
        {/* Ruler */}
        <div className="timeline-ruler-row" style={{ height: RULER_HEIGHT }}>
          <div className="timeline-ruler-label" style={{ width: LABEL_WIDTH }} />
          <div
            ref={rulerRef}
            className="timeline-ruler"
            style={{ width: totalWidth, height: RULER_HEIGHT }}
            onPointerDown={handleRulerPointerDown}
            onPointerMove={handleRulerPointerMove}
          >
            {rulerTicks.map((t) => (
              <div key={t} className="timeline-tick" style={{ left: t * pxPerSec }}>
                <span>{t % 1 === 0 ? `${t}s` : `${t.toFixed(2)}s`}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Lanes */}
        {lanes.map((lane, laneIdx) => (
          <div key={lane.label} className="timeline-lane" style={{ height: LANE_HEIGHT }}>
            <div
              className="timeline-lane-label"
              style={{ width: LABEL_WIDTH, color: lane.color }}
            >
              {lane.label}
            </div>
            <div className="timeline-lane-tokens" style={{ width: totalWidth, height: 28 }}>
              {lane.tokens.map((token, ti) => {
                const left = token.s * pxPerSec;
                const width = Math.max((token.e - token.s) * pxPerSec, 2);
                const isPlaying = currentTime >= token.s && currentTime < token.e;
                const isSelected = selection?.laneIdx === laneIdx && selection?.tokenIdx === ti;
                const isDim = !!(token as LaneToken).dim;

                const borderColor = isSelected || isPlaying
                  ? (isSelected ? lane.color : lane.color + "80")
                  : lane.color + "60";
                const bg = isSelected
                  ? lane.color + "60"
                  : isPlaying
                    ? lane.color + "40"
                    : isDim ? "transparent" : lane.bg;

                return (
                  <div
                    key={ti}
                    className={`timeline-token${isDim ? " dim" : ""}`}
                    style={{
                      left,
                      width,
                      background: bg,
                      border: isDim && !isSelected && !isPlaying
                        ? undefined  // uses .dim class border
                        : `2px solid ${borderColor}`,
                      color: isDim ? undefined : lane.color,
                      outline: isSelected ? `1px solid ${lane.color}` : undefined,
                      outlineOffset: isSelected ? 1 : undefined,
                    }}
                    onMouseEnter={(e) => {
                      const rect = e.currentTarget.getBoundingClientRect();
                      setHover({ laneIdx, tokenIdx: ti, x: rect.left + rect.width / 2, y: rect.bottom + 4 });
                    }}
                    onMouseLeave={() => setHover(null)}
                    onClick={(e) => {
                      e.stopPropagation();
                      selectAndPlay(laneIdx, ti);
                    }}
                  >
                    {token.w}
                  </div>
                );
              })}
            </div>
          </div>
        ))}

        {/* Playhead */}
        <div className="timeline-playhead" style={{ left: playheadX }} />
      </div>

      {/* Hover popover */}
      {hover && (() => {
        const token = lanes[hover.laneIdx]?.tokens[hover.tokenIdx] as LaneToken | undefined;
        if (!token) return null;
        return (
          <div className="timeline-popover" style={{ left: hover.x, top: hover.y }}>
            <div className="word">
              {token.w}{" "}
              <span className="timing">{token.s.toFixed(2)}s &ndash; {token.e.toFixed(2)}s</span>
            </div>
            {token.c != null && (
              <div style={{ color: "var(--text-muted)" }}>conf {token.c.toFixed(3)}</div>
            )}
            {token.editFrom && (
              <div className="edit-detail">
                <table>
                  <tbody>
                    <tr>
                      <td className="label">from</td>
                      <td>
                        <span className="from">{token.editFrom}</span>
                        {token.editFromPhonemes && <span className="phonemes">/{token.editFromPhonemes}/</span>}
                      </td>
                    </tr>
                    <tr>
                      <td className="label">to</td>
                      <td>
                        <span className="to">{token.w}</span>
                        {token.editToPhonemes && <span className="phonemes">/{token.editToPhonemes}/</span>}
                      </td>
                    </tr>
                    {token.editVia && (
                      <tr>
                        <td className="label">via</td>
                        <td>{token.editVia}</td>
                      </tr>
                    )}
                    {token.editSimilarity != null && (
                      <tr>
                        <td className="label">similarity</td>
                        <td className="tabular">{token.editSimilarity.toFixed(3)}</td>
                      </tr>
                    )}
                    {token.editRank != null && (
                      <tr>
                        <td className="label">rank</td>
                        <td className="tabular">{token.editRank.toFixed(3)}</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        );
      })()}
    </div>
  );
}

function findClosestToken(tokens: TimedToken[], time: number): number {
  let best = 0;
  let bestDist = Infinity;
  for (let i = 0; i < tokens.length; i++) {
    const dist = Math.abs(tokens[i].s - time);
    if (dist < bestDist) {
      bestDist = dist;
      best = i;
    }
  }
  return best;
}
