#!/usr/bin/env bash
set -euo pipefail

REMOTE=souffle
REMOTE_ROOT=~/bearcove/hark/asr-synth
TMUX_SESSION=synth-dashboard
TTS_HOST=127.0.0.1
TTS_PORT=3456

echo "=== Syncing to souffle ==="
printf 'y\n' | bash ./sync-to-souffle.sh

echo "=== Rebuilding on souffle ==="
ssh "$REMOTE" "cd $REMOTE_ROOT && cargo build --release -p synth-dashboard"

echo "=== Restarting dashboard in tmux ==="
ssh "$REMOTE" "pkill -f './target/release/synth-dashboard --voice voices/amos2_short.wav --host $TTS_HOST' 2>/dev/null || true"
ssh "$REMOTE" "tmux kill-session -t $TMUX_SESSION 2>/dev/null || true"
ssh "$REMOTE" "tmux new-session -d -s $TMUX_SESSION 'cd $REMOTE_ROOT && exec ./target/release/synth-dashboard --voice voices/amos2_short.wav --host $TTS_HOST'"
ssh "$REMOTE" "sleep 2; lsof -nP -iTCP:$TTS_PORT -sTCP:LISTEN"

echo "=== Ensuring HTTPS proxy ==="
ssh "$REMOTE" "/Applications/Tailscale.app/Contents/MacOS/Tailscale serve --bg --yes --https=443 http://$TTS_HOST:$TTS_PORT"
ssh "$REMOTE" "/Applications/Tailscale.app/Contents/MacOS/Tailscale serve status"

echo
echo "Dashboard is live at: https://souffle.dropbear-piranha.ts.net/#/author"
echo "Attach logs with: ssh souffle tmux attach -t $TMUX_SESSION"
