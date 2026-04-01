import { useEffect, useCallback } from "react";
import type { TimedToken } from "../types";

function formatTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = (s % 60).toFixed(1);
  return `${m}:${sec.padStart(4, "0")}`;
}

function currentWord(tokens: TimedToken[], time: number): string {
  for (const t of tokens) {
    if (time >= t.s && time < t.e) return t.w;
  }
  return "";
}

const ZOOM_LEVELS = [0.25, 0.5, 1, 1.6, 2.6, 3.8, 5.4] as const;

export function EvalPlaybackBar({
  playing,
  currentTime,
  duration,
  zoom,
  tokens,
  onPlayPause,
  onSeek,
  onZoomChange,
}: {
  playing: boolean;
  currentTime: number;
  duration: number;
  zoom: number;
  tokens: TimedToken[];
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

  const word = currentWord(tokens, currentTime);

  return (
    <div className="playback-bar">
      <button className="play-btn" onClick={onPlayPause}>
        {playing ? "\u23F8" : "\u25B6"}
      </button>
      <span className="time">
        {formatTime(currentTime)} / {formatTime(duration)}
      </span>
      <span className={`current-word${word ? "" : " empty"}`}>
        {word || "\u00A0"}
      </span>
      <span className="zoom-label">zoom</span>
      {ZOOM_LEVELS.map((z) => (
        <button
          key={z}
          className={`zoom-btn${zoom === z ? " primary" : ""}`}
          onClick={() => onZoomChange(z)}
        >
          {z}x
        </button>
      ))}
    </div>
  );
}
