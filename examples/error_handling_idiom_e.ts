// Fixture for Consistency.ErrorHandlingIdiom.
//
// The project (a-d.ts) predominantly throws for errors. This file
// returns an error-shaped value instead — the minority idiom, flagged.
export function parseUser(raw: string): unknown {
  if (!raw) {
    return { error: "user payload is empty" };
  }
  // Not flagged — `{ error: null }` is the success arm of a
  // Result-shaped return, not a competing error idiom.
  return { error: null, value: JSON.parse(raw) };
}

interface RaceSocketState {
  error?: string;
}

// Not flagged — spreading `state` derives a new record from an existing
// one (a reducer-style update), not a function signalling failure to its
// caller.
function raceReducer(state: RaceSocketState, message: string): RaceSocketState {
  return { ...state, error: message };
}

// Not flagged — the arrow is a callback argument (a React state setter),
// so its error-shaped body is the callback's value, not this function's
// own error idiom.
function handleFailure(setState: (updater: (p: RaceSocketState) => RaceSocketState) => void, e: unknown) {
  setState((p) => ({ ...p, error: String(e) }));
}
