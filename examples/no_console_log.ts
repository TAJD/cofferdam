// Fixture for Warning.NoConsoleLog.
//
// Default config: only console.log is flagged.
// console.warn / console.error are NOT flagged by default (see methods option).

export function leak(x: unknown) {
  console.log("debug:", x);          // flag (default: log in methods)
  console.warn("warn:", x);          // OK  (not in default methods list)
  console.error("err:", x);          // OK  (not in default methods list)
  console.info("info:", x);          // OK  (not in default methods list)
  console.debug("debug:", x);        // OK  (not in default methods list)
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
