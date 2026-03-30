# Hark State Management Spec

Hark is a push-to-talk dictation tool for macOS. It runs entirely on-device,
captures audio, streams it through an ASR model, and inserts the resulting
text into the frontmost application via a custom IME.

## Architecture

There are two layers:

- **UI layer** — handles hotkey events, drives the overlay, decides when to
  create sessions and how they end. Runs on the main thread.
- **Session** — a self-contained unit of work created for each dictation
  attempt. Owns its capture buffer, ASR handle, and IME handle. Multiple
  sessions can coexist (e.g., the previous one finalizing while a new one
  is streaming).

The UI layer sends exactly three messages to end a session:

- **Abort** — immediate teardown, no trace. It never happened.
- **Cancel** — finalize in the background, create a history entry, but
  clear the IME (don't insert text).
- **Commit(submit: bool)** — finalize, insert text via IME. If submit is
  true, simulate a Return keystroke after insertion.

Once the UI layer ends a session, it is back to Idle and ready for the
next activation. The session finishes its work independently.

## Audio Engine

The audio engine is shared infrastructure. It is not part of any session.
It runs continuously when warm, capturing audio into a circular pre-buffer
that is discarded unless a session taps into it.

### Device management

> h[audio.device.warm-policy]
> Each device has a user-configurable warm/cold policy, persisted by device
> UID. Warm devices have lower activation latency but keep the microphone
> indicator lit and use CPU.

> h[audio.device.active-selection]
> The user may select which input device Hark uses. The selection is
> independent of the system default — Hark MUST NOT change the system
> default recording device.

> h[audio.device.hotplug]
> When input devices appear or disappear, the device list is rebuilt from
> Core Audio. If the selected device disappeared, Hark falls back to the
> system default.

### Engine states

The audio engine has two states: Cold and Warm.

> h[audio.cold]
> When cold, no AVAudioEngine resources are allocated. If a session is
> created while the engine is cold, there will be a brief startup delay.

> h[audio.warm]
> When warm, the AVAudioEngine is running with a tap installed. Incoming
> audio fills a circular pre-buffer (~200ms) that continuously overwrites
> itself. This keeps the microphone ready for instant capture.

Events while Cold:

| Event | Effect |
|---|---|
| Warm-up requested | → Warm (allocate engine, install tap) |
| Device appeared/disappeared | Rebuild device list, stay Cold |

Events while Warm:

| Event | Effect |
|---|---|
| Cool-down requested | → Cold (stop engine, release resources) |
| Device disappeared (active) | Rebuild list, restart engine on fallback device |
| Device appeared/disappeared (other) | Rebuild list, stay Warm |

## UI Layer

The UI layer handles keyboard events and manages the overlay. It creates
sessions and tells them how to end.

### Key event consumption

> h[ui.event-consumption]
> Every key event handled by the UI layer is either **consumed** (the app
> never sees it) or **passthrough** (the app receives it normally). Each
> event is marked explicitly below.

> h[ui.spurious-replay]
> When a Pending activation is determined to be spurious (another key was
> pressed), the other key event MUST be passed through to the app. The
> original ROpt down was already consumed.

### States

```
Idle → Pending → PushToTalk → Idle (commit/cancel)
               → Locked ⇄ LockedOptionHeld → Idle (commit/cancel)
               → Idle (abort)
```

#### Idle

Not recording, no active session.

> h[ui.idle-to-pending]
> On ROpt down: create a new session, transition to Pending.

| Event | Consumed? | Effect |
|---|---|---|
| ROpt down | yes | create session, → Pending |
| RCmd+P | yes | paste last history entry |
| everything else | no | ignored |

#### Pending

ROpt is down. A session has been created and is capturing audio, but the
UI doesn't know yet whether this is a real activation.

> h[ui.pending-to-ptt]
> If ROpt is still held after ~300ms with no other keys pressed: transition
> to PushToTalk.

> h[ui.pending-to-locked]
> If ROpt is released in under ~300ms with no other keys pressed: transition
> to Locked.

> h[ui.pending-abort]
> If any other key is pressed while in Pending: abort the session,
> transition to Idle. The other key MUST be passed through to the app.

| Event | Consumed? | Effect |
|---|---|---|
| ~300ms timer fires | — | → PushToTalk |
| ROpt up (clean) | yes | → Locked |
| any other key down | **no** | abort session, → Idle |

#### PushToTalk

ROpt is held. Recording is confirmed.

> h[ui.ptt-commit]
> On ROpt up while in PushToTalk: commit the session, transition to Idle.

> h[ui.ptt-to-locked]
> On RCmd down while in PushToTalk: transition to Locked. The next ROpt up
> MUST be consumed without committing.

> h[ui.ptt-cancel]
> On Escape while in PushToTalk: cancel the session, transition to Idle.

| Event | Consumed? | Effect |
|---|---|---|
| ROpt up | yes | commit(submit: false), → Idle |
| RCmd down | yes | → Locked (consume next ROpt up) |
| Escape | yes | cancel, → Idle |
| max duration (300s) | — | commit(submit: false), → Idle |
| all other keys | no | passthrough |

#### Locked

Hands-free recording. ROpt is not held. The user may switch apps freely.

> h[ui.locked-option-down]
> On ROpt down while in Locked: transition to LockedOptionHeld.

> h[ui.locked-enter]
> On Enter while in Locked: commit the session with submit, transition to
> Idle. The Enter MUST be consumed — the session will simulate Return
> after text insertion.

> h[ui.locked-esc-passthrough]
> On Escape while in Locked: passthrough to the app. Not intercepted.

| Event | Consumed? | Effect |
|---|---|---|
| ROpt down | yes | → LockedOptionHeld |
| Enter | yes | commit(submit: true), → Idle |
| Escape | **no** | passthrough |
| max duration (300s) | — | commit(submit: false), → Idle |
| all other keys | no | passthrough |

> h[ui.locked-app-switch]
> In Locked and LockedOptionHeld states, the user may switch to other
> applications. Recording continues. The overlay indicates tethering.

#### LockedOptionHeld

In locked mode, ROpt is currently being held.

> h[ui.locked-option-held-commit]
> On ROpt up while in LockedOptionHeld: commit the session, transition to
> Idle.

> h[ui.locked-option-held-cancel]
> On Escape while in LockedOptionHeld: cancel the session, transition to
> Idle. The Escape MUST be consumed.

| Event | Consumed? | Effect |
|---|---|---|
| ROpt up | yes | commit(submit: false), → Idle |
| Escape | yes | cancel, → Idle |
| max duration (300s) | — | commit(submit: false), → Idle |
| all other keys | no | passthrough |

### Max duration

> h[ui.max-duration]
> After 300s of recording, the UI layer commits the current session
> regardless of current state.

### Overlay

The overlay is driven by the UI layer. It reflects the state of the
current session.

> h[ui.overlay.position]
> The overlay is positioned at the IME cursor location.

> h[ui.overlay.tether]
> In locked mode, if the user switches to a different app, the overlay
> indicates that recording is tethered to the original app.

> h[ui.overlay.dismiss]
> Overlay dismissal plays a brief animation before the panel is closed.

## Session

A session is created by the UI layer on ROpt down. It has a unique
identifier and owns three internal layers: Capture, ASR, and IME. Each
layer has its own state machine.

### Session endings

> h[session.abort]
> On abort: all three layers are torn down immediately. No finalization,
> no history entry, no visible side effects. The session never existed
> from the user's perspective.

> h[session.cancel]
> On cancel: capture drains (delivers tail audio), ASR finalizes in the
> background, a history entry is created with the final transcript, but
> the IME clears its marked text without committing. No text is inserted.

> h[session.commit]
> On commit: capture drains (delivers tail audio), ASR finalizes, IME
> commits the final transcript (inserts text). If submit is true, a
> Return keystroke is simulated after insertion.

### Capture layer

The capture layer taps into the shared audio engine to collect audio for
this session.

> h[capture.start]
> When the session is created, the capture layer copies the audio engine's
> pre-buffer and begins accumulating incoming audio. This preserves audio
> from just before the hotkey was pressed.

> h[capture.buffering]
> While buffering, incoming audio samples are appended to the session's
> capture buffer. RMS levels are computed for the overlay level indicator.

> h[capture.drain]
> On commit or cancel, the capture layer enters draining mode. It monitors
> incoming audio for silence (VAD). Drain completes when RMS stays below
> the silence threshold for the required duration, or the drain timeout
> is reached.

> h[capture.drain-delivers]
> When drain completes, all captured samples are delivered to the ASR
> layer. The audio engine remains warm.

> h[capture.abort-discard]
> On abort, the capture layer discards all buffered audio immediately.
> No drain, no delivery.

### ASR layer

The ASR layer processes audio from the capture layer and produces text.

> h[asr.streaming]
> During capture, audio samples are fed to the ASR session incrementally.
> Each feed may return a streaming update with the current transcript.

#### Checkpointing

Without checkpointing, the ASR encoder must re-process an ever-growing
audio buffer on every inference pass, eventually falling behind real-time.
Checkpointing solves this by locking in stable text and resetting the
internal audio/encoder state.

When a checkpoint fires, the checkpointed text is permanent (it will not
change), the internal streaming state is fully reset (audio accumulator,
encoder cache, decoder state), and a new sub-session starts with only the
remaining text as context.

The IME layer does NOT commit on checkpoint — all text (including
checkpointed portions) remains as marked text. Only session commit/cancel
affects the IME.

> h[asr.checkpoint.clause-boundary]
> Checkpoints MUST only occur at clause boundaries: commas, periods,
> exclamation marks, question marks, and equivalent punctuation. A
> checkpoint MUST NOT split in the middle of a clause.

> h[asr.checkpoint.stability]
> A clause boundary is eligible for checkpoint only after the text up to
> that boundary has been identical across multiple consecutive inference
> passes (stability threshold). This prevents checkpointing text that the
> model is still revising.

> h[asr.checkpoint.minimum-length]
> A checkpoint candidate must contain a minimum number of words and
> characters to avoid checkpointing trivial fragments.

> h[asr.checkpoint.no-time-based]
> There MUST NOT be a time-based forced rotation. Checkpoints are
> triggered only by stability at clause boundaries. Time-based rotation
> causes mid-sentence cuts and inserts spurious punctuation.

> h[asr.checkpoint.reset]
> When a checkpoint fires: the checkpointed text moves to the permanent
> transcript, the remainder becomes the new pending prefix, and the
> internal streaming state (audio accumulator, encoder cache, decoder
> state, token IDs) is fully reset. The new sub-session receives the
> trailing text (up to 200 characters) as initial context.

> h[asr.finalize]
> After capture delivers its final samples, the ASR layer runs
> finalization to produce the definitive transcript.

> h[asr.finalize-background]
> Finalization runs in the background. It MUST NOT block the UI layer
> or prevent new sessions from being created.

> h[asr.tail-audio]
> Samples captured after the last streaming feed but before drain
> completion (tail audio) MUST be included in the finalization feed.
> Dropping them causes truncated transcripts.

> h[asr.fallback]
> If finalization produces an empty or suspiciously short result while
> meaningful audio exists, a full-audio batch transcription runs as a
> fallback. The longer result wins.

### IME layer

The IME layer manages text insertion via the harkInput InputMethodKit IME.

> h[ime.activate]
> When the session is created, Hark saves the current input source and
> switches to the harkInput IME.

> h[ime.marked-text]
> During recording, ASR streaming updates are sent to the IME as marked
> text. The text appears underlined in the input field to indicate it is
> provisional. The full transcript (including checkpointed portions)
> remains as marked text — checkpoints do not cause IME commits.

> h[ime.commit]
> On session commit, the marked text is replaced with the final transcript
> via insertText, making it permanent. A trailing space is appended.

> h[ime.clear-on-cancel]
> On session cancel, the marked text is cleared without committing. No
> text is inserted.

> h[ime.deactivate]
> After commit or cancel, the harkInput IME is deactivated and the
> previous input source is restored.

> h[ime.submit]
> When commit has submit: true, a Return keystroke is simulated after a
> short delay following IME deactivation.

> h[ime.abort-teardown]
> On session abort, the IME is deactivated immediately. No commit, no
> clear — the session was never visible to the user.

### Focus loss and parking (IME)

> h[ime.focus-loss-autocommit]
> When the target field loses focus during recording, macOS auto-commits
> the current marked text (standard IME behavior). The IME controller
> saves this text as an auto-committed prefix.

> h[ime.prefix-dedup]
> When streaming resumes after focus return, incoming text is matched
> against the auto-committed prefix. If the text starts with the prefix,
> the prefix is stripped to avoid duplication. If the text has diverged,
> the prefix is discarded.

> h[ime.parking]
> In locked mode, when the user switches away from the target app, the
> IME session is parked: the controller stays alive and isDictating
> remains true so the session resumes on return.

> h[ime.key-intercept]
> While isDictating is true, the IME intercepts Enter (triggers submit)
> and Escape (triggers cancel) via distributed notifications back to the
> main app.

> h[ime.communication]
> The main app communicates with the IME via distributed notifications:
> setMarkedText, commitText, cancelInput, stopDictating.

## Hotkey

> h[hotkey.right-option]
> The hotkey is the Right Option key, detected via CGEvent tap. Key-down
> and key-up events on ROpt drive all UI layer transitions.

## Media

> h[media.pause-on-record]
> When media pause is enabled, Hark detects active audio output from other
> apps and sends a pause command before recording starts.

> h[media.resume-after-record]
> After recording ends, Hark resumes media playback only if it was the one
> that paused it.

## Language Detection

Language is always auto-detected. There are no overrides.

> h[lang.detect-from-ax]
> At session creation, Hark walks the AX element tree of the focused
> window (up to 2000 elements), collects text, takes the last 500
> characters, and runs NLLanguageRecognizer.

> h[lang.confidence-threshold]
> English requires 50% confidence. Non-English languages require 80%
> confidence. Below the threshold, no language hint is passed to the model.

> h[lang.lock-during-streaming]
> If no language was determined at session start, the first streaming
> update that reports a detected language locks the session to that
> language for the remainder of the recording.

## History

> h[history.paste-last]
> RCmd+P pastes the last history entry into the current app.

## Coordination

> h[coord.drain-before-finalize]
> Capture drain MUST complete and deliver all samples before ASR
> finalization begins.

> h[coord.reactivate-locked-app]
> On commit in locked mode, if the user is in a different app, Hark
> brings the original app to the front and waits before inserting text.
