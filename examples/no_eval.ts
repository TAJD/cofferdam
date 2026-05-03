// Fixture for Warning.NoEval.

export function direct(code: string) {
  return eval(code);                  // flag: bare eval call
}

export function indirect(body: string) {
  return new Function(body);          // flag: new Function constructor
}

export function combined(code: string, body: string) {
  eval(code);                         // flag
  return new Function("a", "b", body); // flag (multi-arg constructor)
}

// Identifiers shadowed locally should still flag — bare-identifier
// match is the line we draw without type info.
export function shadowed() {
  function eval(_: string) {
    return 42;
  }
  return eval("ignored");             // flag (bare identifier)
}

// Property access NOT flagged — only top-level eval / new Function.
export class Sandbox {
  eval(_: string) { return 0; }
}
const s = new Sandbox();
export const result = s.eval("ok");   // OK (method, not bare eval)

// `Function` used as a type or factory access NOT flagged — only
// `new Function(...)` constructor calls trigger.
export function fnRef(): typeof Function {
  return Function;                    // OK (member access)
}
