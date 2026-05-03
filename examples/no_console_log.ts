// Fixture for Warning.NoConsoleLog.

export function leak(x: unknown) {
  console.log("debug:", x);          // flag
  console.warn("warn:", x);          // flag
  console.error("err:", x);          // flag
  console.info("info:", x);          // flag
  console.debug("debug:", x);        // flag
  return x;
}

export function nested(value: string) {
  if (value.length > 0) {
    console.log(value);              // flag (inside if)
  }
}

// Property access without a call should NOT flag.
export const ref = console.log;       // OK (member, no call)

// Aliased calls escape detection — known limit.
const c = console;
export function aliased(x: unknown) {
  c.log(x);                          // OK (aliased — bare-identifier match only)
}

// `Console.log` capitalised should NOT flag — different identifier.
class Console {
  static log(_: unknown) {}
}
export function namedConsole(x: unknown) {
  Console.log(x);                    // OK
}
