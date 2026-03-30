import { useState, useRef, useCallback } from "react";

type RecorderState = "idle" | "recording" | "processing";

export function useAudioRecorder() {
  const [state, setState] = useState<RecorderState>("idle");
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);

  const start = useCallback(async () => {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const mr = new MediaRecorder(stream, { mimeType: "audio/webm" });
    chunksRef.current = [];
    mr.ondataavailable = (e) => {
      if (e.data.size > 0) chunksRef.current.push(e.data);
    };
    mr.start();
    mediaRecorderRef.current = mr;
    setState("recording");
  }, []);

  const stop = useCallback((): Promise<Blob> => {
    return new Promise((resolve) => {
      const mr = mediaRecorderRef.current;
      if (!mr) {
        resolve(new Blob());
        return;
      }
      mr.onstop = () => {
        const blob = new Blob(chunksRef.current, { type: "audio/wav" });
        mr.stream.getTracks().forEach((t) => t.stop());
        setState("idle");
        resolve(blob);
      };
      mr.stop();
      setState("processing");
    });
  }, []);

  return { state, start, stop };
}
