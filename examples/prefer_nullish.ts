// Fixture for Refactor.PreferNullishCoalescing.

interface Config {
  timeout?: number;
  name?: string;
  debug?: boolean;
  tags?: string[];
  meta?: Record<string, string>;
}

export function defaults(cfg: Config) {
  const timeout = cfg.timeout || 5000;       // flag: member || number
  const name = cfg.name || "anonymous";      // flag: member || string
  const debug = cfg.debug || false;          // flag: member || bool
  const tags = cfg.tags || [];               // flag: member || array literal
  const meta = cfg.meta || {};               // flag: member || object literal
  return { timeout, name, debug, tags, meta };
}

export function nullishLiterals(cfg: Config) {
  const a = cfg.name || null;                // flag (RHS is null literal)
  const b = cfg.name || undefined;           // flag (RHS is undefined ident)
  return { a, b };
}

export function computedMember(map: Record<string, number>, key: string) {
  return map[key] || 0;                      // flag: computed member || number
}

// Bare-identifier LHS — NOT flagged (could be alt branch).
export function altBranch(flag: boolean, fallback: boolean) {
  return flag || fallback;                   // OK
}

// Function-call LHS — NOT flagged (return type ambiguous).
export function callLhs(getValue: () => number | null) {
  return getValue() || 0;                    // OK
}

// Arithmetic LHS — NOT flagged (clearly wants falsy fallthrough).
export function arithmetic(a: number, b: number) {
  return (a + b) || 1;                       // OK
}

// Member || identifier — NOT flagged (RHS not a default literal).
export function memberOrIdent(cfg: Config, fallback: string) {
  return cfg.name || fallback;               // OK (RHS bare ident)
}

// Member || function call — NOT flagged.
export function memberOrCall(cfg: Config, getDefault: () => string) {
  return cfg.name || getDefault();           // OK (RHS is call)
}
