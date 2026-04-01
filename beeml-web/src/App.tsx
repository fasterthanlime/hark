import "./index.css";
import { TranscribeDemoPanel } from "./components/TranscribeDemoPanel";

export default function App() {
  return (
    <div style={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: "1rem",
          padding: "0.5rem 1rem",
          borderBottom: "1px solid var(--border)",
          background: "var(--bg-surface)",
        }}
      >
        <strong>beeml-web</strong>
        <span style={{ color: "var(--text-muted)", fontSize: "0.85rem" }}>
          Transcribe demo
        </span>
      </header>
      <main style={{ flex: 1, overflow: "hidden", display: "flex", flexDirection: "column" }}>
        <TranscribeDemoPanel />
      </main>
    </div>
  );
}
