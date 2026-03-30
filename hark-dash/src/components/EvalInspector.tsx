import { useRef, useState, useCallback, useEffect } from "react";
import type { EvalInspectorData } from "../types";
import { EvalPlaybackBar } from "./EvalPlaybackBar";
import { EvalTimeline } from "./EvalTimeline";
import { TranscriptComparison } from "./TranscriptComparison";

export function EvalInspector({
  data,
  audioUrl,
}: {
  data: EvalInspectorData;
  audioUrl?: string;
}) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [zoom, setZoom] = useState(1);
  const rafRef = useRef<number>(0);

  // Set up audio element
  useEffect(() => {
    if (!audioUrl) return;
    const audio = new Audio(audioUrl);
    audioRef.current = audio;

    audio.addEventListener("loadedmetadata", () => setDuration(audio.duration));
    audio.addEventListener("ended", () => setPlaying(false));

    return () => {
      audio.pause();
      audio.src = "";
      audioRef.current = null;
    };
  }, [audioUrl]);

  // Animation frame loop for time updates
  useEffect(() => {
    if (!playing) return;
    const tick = () => {
      const audio = audioRef.current;
      if (audio) setCurrentTime(audio.currentTime);
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafRef.current);
  }, [playing]);

  const handlePlayPause = useCallback(() => {
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) {
      audio.play();
      setPlaying(true);
    } else {
      audio.pause();
      setPlaying(false);
    }
  }, []);

  const handleSeek = useCallback((time: number) => {
    const audio = audioRef.current;
    if (audio) audio.currentTime = time;
    setCurrentTime(time);
  }, []);

  return (
    <div style={{ display: "flex", flexDirection: "column", flex: 1, overflow: "hidden" }}>
      {/* Playback bar */}
      {audioUrl && (
        <EvalPlaybackBar
          playing={playing}
          currentTime={currentTime}
          duration={duration}
          zoom={zoom}
          onPlayPause={handlePlayPause}
          onSeek={handleSeek}
          onZoomChange={setZoom}
        />
      )}

      {/* Timeline */}
      <EvalTimeline
        alignments={data.alignments}
        parakeetAlignment={data.parakeetAlignment}
        currentTime={currentTime}
        duration={duration}
        onSeek={handleSeek}
        zoom={zoom}
      />

      {/* Scrollable detail area */}
      <div style={{ flex: 1, overflow: "auto", padding: "1rem" }}>
        {data.elapsedMs != null && (
          <div style={{ fontSize: "0.8rem", color: "var(--text-muted)", marginBottom: "0.75rem" }}>
            {(data.elapsedMs / 1000).toFixed(2)}s total
          </div>
        )}

        <TranscriptComparison
          transcriptLabel={data.transcriptLabel}
          transcript={data.transcript}
          expected={data.expected}
          corrected={data.prototype.corrected}
          accepted={data.prototype.accepted}
        />
      </div>
    </div>
  );
}
