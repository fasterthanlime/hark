import { useEffect, useCallback } from "react";

function formatTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = (s % 60).toFixed(1);
  return `${m}:${sec.padStart(4, "0")}`;
}

const ZOOM_LEVELS = [0.25, 0.5, 1, 1.6, 2.6, 3.8, 5.4] as const;

export function EvalPlaybackBar({
  playing,
  currentTime,
  duration,
  zoom,
  onPlayPause,
  onSeek,
  onZoomChange,
}: {
  playing: boolean;
  currentTime: number;
  duration: number;
  zoom: number;
  onPlayPause: () => void;
  onSeek: (time: number) => void;
  onZoomChange: (z: number) => void;
}) {
  const step = useCallback(
    (delta: number) => {
      onSeek(Math.max(0, Math.min(currentTime + delta, duration)));
    },
    [currentTime, duration, onSeek],
  );

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;

      switch (e.code) {
        case "Space":
          e.preventDefault();
          onPlayPause();
          break;
        case "ArrowLeft":
          e.preventDefault();
          step(e.shiftKey ? -1 : -0.1);
          break;
        case "ArrowRight":
          e.preventDefault();
          step(e.shiftKey ? 1 : 0.1);
          break;
        case "Home":
          e.preventDefault();
          onSeek(0);
          break;
        case "End":
          e.preventDefault();
          onSeek(duration);
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onPlayPause, step, onSeek, duration]);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.75rem",
        padding: "0.4rem 1rem",
        background: "var(--bg-surface)",
        borderBottom: "1px solid var(--border)",
        fontSize: "0.85rem",
      }}
    >
      <button onClick={onPlayPause} style={{ minWidth: 36 }}>
        {playing ? "⏸" : "▶"}
      </button>
      <span style={{ fontVariantNumeric: "tabular-nums", color: "var(--text-muted)" }}>
        {formatTime(currentTime)} / {formatTime(duration)}
      </span>
      <input
        type="range"
        min={0}
        max={duration || 1}
        step={0.01}
        value={currentTime}
        onChange={(e) => onSeek(parseFloat(e.target.value))}
        style={{ flex: 1, minWidth: 100 }}
      />
      <span style={{ fontSize: "0.75rem", color: "var(--text-muted)" }}>zoom</span>
      {ZOOM_LEVELS.map((z) => (
        <button
          key={z}
          className={zoom === z ? "primary" : ""}
          onClick={() => onZoomChange(z)}
          style={{ padding: "0.2em 0.5em", fontSize: "0.75rem" }}
        >
          {z}x
        </button>
      ))}
    </div>
  );
}
