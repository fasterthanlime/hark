import "./index.css";
import { useState } from "react";
import { JudgeEvalPanel } from "./components/JudgeEvalPanel";
import { JudgeRapidFirePanel } from "./components/JudgeRapidFirePanel";
import { RetrievalPrototypeLab } from "./components/RetrievalPrototypeLab";
import { TranscribeDemoPanel } from "./components/TranscribeDemoPanel";

const WS_URL = "ws://127.0.0.1:9944";

export default function App() {
  const [tab, setTab] = useState<
    "transcribe" | "retrieval" | "rapid-fire" | "judge-eval"
  >("rapid-fire");

  return (
    <div className="app-shell">
      <header className="app-header">
        <strong>beeml</strong>
        <div className="tab-row" role="tablist">
          <button
            role="tab"
            aria-selected={tab === "rapid-fire"}
            className={tab === "rapid-fire" ? "primary" : ""}
            onClick={() => setTab("rapid-fire")}
          >
            Rapid Fire
          </button>
          <button
            role="tab"
            aria-selected={tab === "judge-eval"}
            className={tab === "judge-eval" ? "primary" : ""}
            onClick={() => setTab("judge-eval")}
          >
            Judge Eval
          </button>
          <button
            role="tab"
            aria-selected={tab === "retrieval"}
            className={tab === "retrieval" ? "primary" : ""}
            onClick={() => setTab("retrieval")}
          >
            Retrieval Lab
          </button>
          <button
            role="tab"
            aria-selected={tab === "transcribe"}
            className={tab === "transcribe" ? "primary" : ""}
            onClick={() => setTab("transcribe")}
          >
            Transcribe
          </button>
        </div>
      </header>
      <main className="app-main">
        <div className="app-page">
          {tab === "transcribe" ? (
            <TranscribeDemoPanel wsUrl={WS_URL} />
          ) : tab === "rapid-fire" ? (
            <JudgeRapidFirePanel wsUrl={WS_URL} />
          ) : tab === "judge-eval" ? (
            <JudgeEvalPanel wsUrl={WS_URL} />
          ) : (
            <RetrievalPrototypeLab wsUrl={WS_URL} />
          )}
        </div>
      </main>
    </div>
  );
}
