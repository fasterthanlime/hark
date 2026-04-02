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

## Sleeps used as proxy for acknowledgement in finishIME

**Location:** `Session.swift` `finishIME`

After `commitText`, we sleep 50ms before calling `deactivate()`, hoping the commit has propagated. Same before `simulateReturn()`. These sleeps are fragile:
- On the Session actor, `await Task.sleep` makes the actor re-entrant — other methods can interleave and mutate state during the sleep.
- If the task gets cancelled, the sleep throws `CancellationError` (now caught, previously swallowed by `try?`).

**Proper fix:** Replace sleeps with explicit acknowledgement. `commitText` should be an async operation that resolves when the IME confirms the text was inserted, then deactivate. The broker already has the plumbing for this — add a reply to the commit XPC call.

## Actor reentrancy across all awaits in Session

**Location:** `Session` actor, all methods with `await`

Every `await` in the Session actor (sleeps, MainActor.run, XPC calls) allows other methods to enter and mutate state. This means:
- `abort()` can fire during `finishIME()`'s sleep
- `routeDidBecomeActive()` can fire during `start()`'s `await activate()`
- State checks before an `await` may be stale after it

**Proper fix:** Critical state transitions should be atomic (no awaits between check and mutation). For multi-step operations, use a serial queue of operations or a state machine that rejects invalid transitions rather than relying on timing.

## handleDidActivateApplication suppressed during IME activation

**Location:** `AppState.swift` `handleDidActivateApplication`

During IME activation (including the focus cycle fallback), app-switch notifications are suppressed to prevent the focus cycle from aborting its own session. This means genuine app switches during activation are also ignored.

**Proper fix:** The focus cycle should signal its intent explicitly (e.g. a flag) rather than relying on `imeSessionState == .activating` to suppress all app-switch handling.
