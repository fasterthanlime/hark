# Tech Debt

## IME activation timeout is a hardcoded hack

**Location:** `AppState.swift` `startIMEAckTimeoutIfNeeded`

The 500ms timeout before firing the focus cycle fallback is measured from hotkey down, not from when `TISSelectInputSource` is actually called. The session setup (create session, prepareSession XPC, 20ms deferred selection) takes ~180ms, so the effective wait after TIS is only ~320ms.

**Proper fix:** Start the timeout from when `TISSelectInputSource` succeeds, not from hotkey down. This requires the `activate()` call in `BeeInputClient` to signal back to `AppState` that TIS selection happened, so the timeout can start from the right moment.

## Focus cycle (hide/reactivate) as fallback for unreliable TISSelectInputSource

**Location:** `BeeInputClient.forceFocusCycle`, called from `AppState.startIMEAckTimeoutIfNeeded`

`TISSelectInputSource` does not reliably trigger `activateServer` on the IME. As a fallback, we hide the target app for 200ms then reactivate it, forcing a real focus change. This is visible to the user (app briefly disappears).

**Proper fix:** Find a less invasive way to force the OS to call `activateServer`. Candidates:
- Accessibility API to poke focus without hiding
- A different TIS API that forces re-evaluation
- Keeping bee selected permanently (requires solving keyboard switching UX)

## Deferred claim in IME is a timing workaround

**Location:** `BeeInputController.activateServer` — 60ms `DispatchWorkItem` delay before claiming

The OS sometimes sends `activateServer` immediately followed by `deactivateServer` (~1ms apart). The 60ms deferral prevents burning the prepared session on a spurious activation. If `deactivateServer` fires within 60ms, the claim is cancelled and the session stays prepared for retry.

**Proper fix:** Understand why the OS sends spurious activate/deactivate pairs and prevent them at the source, or find an API that gives a reliable "input method is truly active" signal.
