import { useRef, useEffect, useCallback } from "react";
import type { PrototypeAlignments, TimedToken } from "../types";

type Lane = {
  label: string;
  tokens: TimedToken[];
  color: string;
  bg: string;
};

function buildLanes(alignments: PrototypeAlignments, parakeetAlignment: TimedToken[]): Lane[] {
  const lanes: Lane[] = [];

  if (parakeetAlignment.length > 0) {
    lanes.push({
      label: "Parakeet",
      tokens: parakeetAlignment,
      color: "var(--lane-parakeet)",
      bg: "var(--lane-parakeet-bg)",
    });
  }

  if (alignments.espeak && alignments.espeak.length > 0) {
    lanes.push({
      label: "eSpeak",
      tokens: alignments.espeak,
      color: "var(--lane-espeak)",
      bg: "var(--lane-espeak-bg)",
    });
  }

  if (alignments.zipaEspeak && alignments.zipaEspeak.length > 0) {
    lanes.push({
      label: "ZIPA@eSpeak",
      tokens: alignments.zipaEspeak,
      color: "var(--lane-zipa-espeak)",
      bg: "var(--lane-zipa-espeak-bg)",
    });
  }

  if (alignments.zipa && alignments.zipa.length > 0) {
    lanes.push({
      label: "ZIPA",
      tokens: alignments.zipa,
      color: "var(--lane-zipa)",
      bg: "var(--lane-zipa-bg)",
    });
  }

  return lanes;
}

function getTimeRange(lanes: Lane[]): [number, number] {
  let min = Infinity;
  let max = 0;
  for (const lane of lanes) {
    for (const t of lane.tokens) {
      if (t.s < min) min = t.s;
      if (t.e > max) max = t.e;
    }
  }
  if (!isFinite(min)) return [0, 1];
  return [min, max];
}

export function EvalTimeline({
  alignments,
  parakeetAlignment,
  currentTime,
  duration,
  onSeek,
  zoom = 1,
}: {
  alignments: PrototypeAlignments;
  parakeetAlignment: TimedToken[];
  currentTime: number;
  duration: number;
  onSeek: (time: number) => void;
  zoom?: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const lanes = buildLanes(alignments, parakeetAlignment);
  const [timeStart, timeEnd] = getTimeRange(lanes);
  const totalDuration = Math.max(duration, timeEnd, 0.01);
  const pixelsPerSecond = 120 * zoom;
  const totalWidth = totalDuration * pixelsPerSecond;
  const labelWidth = 90;

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = e.clientX - rect.left - labelWidth;
      if (x < 0) return;
      const time = (x / (totalWidth)) * totalDuration;
      onSeek(Math.max(0, Math.min(time, totalDuration)));
    },
    [totalWidth, totalDuration, onSeek],
  );

  // Auto-scroll to keep playhead visible
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const playheadX = labelWidth + (currentTime / totalDuration) * totalWidth;
    const scrollLeft = el.scrollLeft;
    const viewWidth = el.clientWidth;
    if (playheadX < scrollLeft + labelWidth + 20 || playheadX > scrollLeft + viewWidth - 20) {
      el.scrollLeft = playheadX - viewWidth / 2;
    }
  }, [currentTime, totalDuration, totalWidth]);

  if (lanes.length === 0) {
    return (
      <div
        style={{
          padding: "2rem",
          textAlign: "center",
          color: "var(--text-muted)",
          background: "var(--bg-surface)",
          borderBottom: "1px solid var(--border)",
        }}
      >
        No alignment data available
      </div>
    );
  }

  const playheadX = labelWidth + (currentTime / totalDuration) * totalWidth;

  return (
    <div
      ref={containerRef}
      onClick={handleClick}
      style={{
        width: "100%",
        overflowX: "auto",
        overflowY: "hidden",
        background: "var(--bg-surface)",
        borderBottom: "1px solid var(--border)",
        cursor: "crosshair",
        position: "relative",
      }}
    >
      <div style={{ width: labelWidth + totalWidth, minHeight: lanes.length * 36 + 20, position: "relative", padding: "10px 0" }}>
        {/* Time ruler */}
        <div style={{ position: "absolute", top: 0, left: labelWidth, width: totalWidth, height: "100%", pointerEvents: "none" }}>
          {Array.from({ length: Math.ceil(totalDuration) + 1 }, (_, i) => (
            <div
              key={i}
              style={{
                position: "absolute",
                left: (i / totalDuration) * totalWidth,
                top: 0,
                height: "100%",
                borderLeft: "1px solid var(--border)",
                opacity: 0.3,
              }}
            >
              <span style={{ fontSize: "0.65rem", color: "var(--text-dim)", position: "absolute", top: 0, left: 3 }}>
                {i}s
              </span>
            </div>
          ))}
        </div>

        {/* Lanes */}
        {lanes.map((lane) => (
          <div key={lane.label} style={{ display: "flex", alignItems: "center", height: 36, position: "relative" }}>
            <div
              style={{
                width: labelWidth,
                paddingLeft: 12,
                fontSize: "0.75rem",
                fontWeight: 600,
                color: lane.color,
                flexShrink: 0,
                position: "sticky",
                left: 0,
                zIndex: 2,
                background: "var(--bg-surface)",
              }}
            >
              {lane.label}
            </div>
            <div style={{ position: "relative", width: totalWidth, height: 28 }}>
              {lane.tokens.map((token, ti) => {
                const left = ((token.s - timeStart) / totalDuration) * totalWidth;
                const width = Math.max(((token.e - token.s) / totalDuration) * totalWidth, 2);
                return (
                  <div
                    key={ti}
                    title={`${token.w} (${token.s.toFixed(2)}s–${token.e.toFixed(2)}s${token.c != null ? `, conf ${token.c.toFixed(2)}` : ""})`}
                    style={{
                      position: "absolute",
                      left,
                      width,
                      top: 2,
                      height: 24,
                      background: lane.bg,
                      border: `1px solid ${lane.color}40`,
                      borderRadius: 3,
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      fontSize: "0.7rem",
                      color: lane.color,
                      overflow: "hidden",
                      whiteSpace: "nowrap",
                      textOverflow: "ellipsis",
                      padding: "0 2px",
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
        <div
          style={{
            position: "absolute",
            left: playheadX,
            top: 0,
            bottom: 0,
            width: 2,
            background: "var(--accent)",
            zIndex: 10,
            pointerEvents: "none",
          }}
        />
      </div>
    </div>
  );
}
