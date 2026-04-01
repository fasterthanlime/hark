import "./index.css";
import { TranscribeDemoPanel } from "./components/TranscribeDemoPanel";

export default function App() {
  return (
    <div className="app-shell">
      <header className="app-header">
        <strong>beeml-web</strong>
        <span className="subtitle">Transcribe demo</span>
      </header>
      <main className="app-main">
        <TranscribeDemoPanel />
      </main>
    </div>
  );
}
