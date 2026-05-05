// Refactor.LongAndComplex — flags only `tangled`, not `flatLong` or `shortBranchy`.
// Defaults: length_limit=75, cyclomatic_limit=15.

// Long but flat — 80 lines of straight-line config. Length passes the
// MaxFunctionLength bar but cyclomatic is 1, so LongAndComplex stays quiet.
export function flatLong() {
  const config = {
    a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10,
    k: 11, l: 12, m: 13, n: 14, o: 15, p: 16, q: 17, r: 18, s: 19, t: 20,
    u: 21, v: 22, w: 23, x: 24, y: 25, z: 26,
  };
  const more = {
    aa: 1, ab: 2, ac: 3, ad: 4, ae: 5, af: 6, ag: 7, ah: 8, ai: 9, aj: 10,
    ak: 11, al: 12, am: 13, an: 14, ao: 15, ap: 16, aq: 17, ar: 18, as: 19, at: 20,
    au: 21, av: 22, aw: 23, ax: 24, ay: 25, az: 26,
  };
  const yetMore = {
    ba: 1, bb: 2, bc: 3, bd: 4, be: 5, bf: 6, bg: 7, bh: 8, bi: 9, bj: 10,
    bk: 11, bl: 12, bm: 13, bn: 14, bo: 15, bp: 16, bq: 17, br: 18, bs: 19, bt: 20,
    bu: 21, bv: 22, bw: 23, bx: 24, by: 25, bz: 26,
  };
  const evenMore = {
    ca: 1, cb: 2, cc: 3, cd: 4, ce: 5, cf: 6, cg: 7, ch: 8, ci: 9, cj: 10,
    ck: 11, cl: 12, cm: 13, cn: 14, co: 15, cp: 16, cq: 17, cr: 18, cs: 19, ct: 20,
    cu: 21, cv: 22, cw: 23, cx: 24, cy: 25, cz: 26,
  };
  return { config, more, yetMore, evenMore };
}

// Short but branchy — high cyclomatic in a short body. CyclomaticComplexity
// flags it; LongAndComplex stays quiet because it's not long.
export function shortBranchy(x: number, y: number, z: number, mode: string): string {
  if (x > 0 && y > 0) {
    if (z > 0 || mode === "all") {
      if (x === y && y === z) return "eq";
      if (x > y || y > z) return "asc";
    }
  }
  switch (mode) {
    case "a": return "A";
    case "b": return "B";
    case "c": return "C";
    case "d": return "D";
    case "e": return "E";
    default: return "?";
  }
}

// Long AND complex — should be the only function flagged by LongAndComplex
// at the production defaults (length_limit=75, cyclomatic_limit=15).
// Constructed to clear both with a comfortable margin so threshold tuning
// in either dimension doesn't silently drop the test.
export function tangled(x: number, y: number, z: number, mode: string, env: string): string {
  let acc = 0;
  if (x > 0 && y > 0) {
    if (z > 0 || mode === "all") {
      for (let i = 0; i < x; i++) {
        if (i % 2 === 0) {
          while (i > 0 && i < y) {
            i++;
            acc += 1;
          }
        }
        if (i % 3 === 0 && acc > 10) {
          acc -= 1;
        }
        if (i % 5 === 0 && env === "prod") {
          acc *= 2;
        }
      }
      for (let j = 0; j < y; j++) {
        if (j % 2 === 1) {
          acc += j;
        } else {
          acc -= j;
        }
        if (j > z && mode !== "test") {
          acc += 100;
        }
      }
    }
  }
  if (env === "prod" && mode !== "test") {
    for (const key of Object.keys({})) {
      if (key.startsWith("_") || key.length > 5) {
        acc += key.length;
      }
      if (key.endsWith("_id") && acc < 1000) {
        acc += 1;
      }
    }
  }
  if (env === "staging" || env === "preview") {
    for (const k of Object.keys({})) {
      if (k.length > 3 && k !== "skip") {
        acc -= 1;
      }
    }
  }
  if (mode === "x" && env === "y") acc += 1;
  if (mode === "y" && env === "z") acc += 2;
  if (mode === "z" && env === "x") acc += 3;
  if (mode === "p" && env === "q") acc += 4;
  if (mode === "q" && env === "p") acc += 5;
  while (acc > 1000) {
    if (acc % 2 === 0) {
      acc = acc / 2;
    } else {
      acc = acc - 1;
    }
  }
  do {
    acc -= 1;
  } while (acc > 100 && env !== "stop");
  try {
    if (acc < 0) {
      throw new Error("negative");
    }
  } catch (e) {
    acc = 0;
  }
  switch (mode) {
    case "a":
      if (acc > 0) return "A+";
      return "A";
    case "b":
      if (acc > 0 && env === "prod") return "B+";
      return "B";
    case "c":
      return acc > 0 ? "C+" : "C";
    case "d":
      return acc > 10 ? "D+" : "D";
    case "e":
      return acc > 20 && env === "prod" ? "E+" : "E";
    case "f":
      return "F";
    case "g":
      return "G";
    case "h":
      return "H";
    default:
      return acc > 0 ? "?+" : "?";
  }
}

// Comment-heavy — should NOT trip MaxFunctionLength even though raw line
// span is large. Effective code lines are well under 50.
//
//
//
//
//
//
//
//
export function commentHeavy(): number {
  // line of comment 1
  // line of comment 2
  // line of comment 3
  // line of comment 4
  // line of comment 5
  // line of comment 6
  // line of comment 7
  // line of comment 8
  // line of comment 9
  // line of comment 10
  // line of comment 11
  // line of comment 12
  // line of comment 13
  // line of comment 14
  // line of comment 15
  // line of comment 16
  // line of comment 17
  // line of comment 18
  // line of comment 19
  // line of comment 20
  // line of comment 21
  // line of comment 22
  // line of comment 23
  // line of comment 24
  // line of comment 25
  // line of comment 26
  // line of comment 27
  // line of comment 28
  // line of comment 29
  // line of comment 30
  // line of comment 31
  // line of comment 32
  // line of comment 33
  // line of comment 34
  // line of comment 35
  return 1;
}
