// Fixture for Warning.NoDebugger.

export function pause(x: unknown) {
  debugger;                          // flag
  return x;
}

export function nestedPause(items: unknown[]) {
  for (const item of items) {
    if (item) {
      debugger;                      // flag (inside nested blocks)
    }
  }
}

// Strings containing "debugger" should NOT flag — only the
// DebuggerStatement form matters.
export const text = "debugger";       // OK
export const comment = "// debugger"; // OK
