#!/usr/bin/env node
// scripts/gen-synthetic-repo.mjs — deterministic synthetic TypeScript repo
// generator for engine benchmarking (CD-184).
//
// CD-165's target (warm-cache p50 <=2s / p95 <=5s on a 5k-file repo) can't be
// measured against the real repos we have on hand (476 and 252 files), so this
// builds one at the target scale. The output is deliberately *not* N copies of
// one file: a degenerate fixture makes the cross-file quadratic checks look
// either far worse or far better than reality.
//
// What it stresses, and why the knobs are shaped the way they are:
//   - Design.DuplicateTypeShape — interface/type-alias declarations grouped
//     into "families". Members of a family are exact copies, near-duplicates
//     that clear the 0.8 similarity threshold (one added field, one retyped
//     field, one dropped field), or divergent enough to fall below it. Most
//     families are singletons carrying a family-unique marker field, so they
//     can never match anything: that keeps the duplicate *rate* realistic
//     while the pairwise comparison cost still scales with the total count.
//   - Refactor.NearDuplicateBlock — 6+ statement windows built from a handful of
//     templates. Local identifiers vary per occurrence (the check canonicalises
//     them away, so duplicates still match); numeric literals do not (they are
//     hashed verbatim), so the literal set is what actually decides whether two
//     blocks collide.
//   - Design.DuplicateExportName / ImportFanOutOutlier / OrphanExport — a real
//     import graph over a nested directory tree, with a deliberate tail of
//     re-used export names.
//   - Refactor.LongAndComplex / nesting checks — function bodies vary in
//     length, branchiness and nesting depth.
//
// Usage:
//   node scripts/gen-synthetic-repo.mjs --out <dir> [--files 5000] [--seed 42]
//
// Determinism: same --files/--seed/--out produces byte-identical output. All
// randomness goes through a seeded mulberry32 PRNG; nothing reads the clock.

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, posix, resolve } from "node:path";
import { argv, exit, stdout } from "node:process";

// ─── args ──────────────────────────────────────────────────────────────────

function parseArgs(args) {
  const opts = {
    out: null,
    files: 5000,
    seed: 42,
    // Probability that a new type-shape family is a duplicate family (size
    // 2-4) rather than a singleton. 0.08 lands ~20% of all shapes inside a
    // duplicate family, which is high-ish for real code but keeps the check
    // producing signal rather than nothing.
    typeFamilyRate: 0.08,
    // Same idea for duplicate code blocks.
    blockFamilyRate: 0.1,
    clean: true,
  };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    const next = () => args[++i];
    switch (arg) {
      case "--out": opts.out = next(); break;
      case "--files": opts.files = Number(next()); break;
      case "--seed": opts.seed = Number(next()); break;
      case "--type-family-rate": opts.typeFamilyRate = Number(next()); break;
      case "--block-family-rate": opts.blockFamilyRate = Number(next()); break;
      case "--no-clean": opts.clean = false; break;
      case "--help":
      case "-h":
        stdout.write(
          "usage: gen-synthetic-repo.mjs --out <dir> [--files N] [--seed N]\n" +
          "       [--type-family-rate F] [--block-family-rate F] [--no-clean]\n",
        );
        exit(0);
        break;
      default:
        console.error(`unknown argument: ${arg}`);
        exit(2);
    }
  }
  if (!opts.out) {
    console.error("--out <dir> is required");
    exit(2);
  }
  if (!Number.isInteger(opts.files) || opts.files < 1) {
    console.error("--files must be a positive integer");
    exit(2);
  }
  return opts;
}

// ─── seeded PRNG ───────────────────────────────────────────────────────────

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const int = (rng, lo, hi) => lo + Math.floor(rng() * (hi - lo + 1));
const pick = (rng, arr) => arr[Math.floor(rng() * arr.length)];

function shuffle(rng, arr) {
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [arr[i], arr[j]] = [arr[j], arr[i]];
  }
  return arr;
}

// ─── vocabulary ────────────────────────────────────────────────────────────

const FIELD_NAMES = [
  "id", "name", "label", "title", "slug", "kind", "status", "state", "phase",
  "createdAt", "updatedAt", "deletedAt", "expiresAt", "startedAt", "endedAt",
  "ownerId", "userId", "accountId", "tenantId", "parentId", "sessionId",
  "email", "phone", "address", "city", "country", "postcode", "region",
  "amount", "total", "subtotal", "discount", "tax", "fee", "balance",
  "currency", "rate", "quantity", "weight", "height", "width", "depth",
  "score", "rank", "weightings", "priority", "severity", "confidence",
  "enabled", "visible", "archived", "locked", "verified", "draft",
  "tags", "labels", "categories", "keywords", "aliases", "flags",
  "url", "path", "host", "port", "scheme", "query", "fragment",
  "version", "revision", "checksum", "etag", "digest", "signature",
  "message", "reason", "detail", "hint", "code", "trace",
  "attempts", "retries", "timeout", "backoff", "interval", "cursor",
  "locale", "timezone", "currencyCode", "unit", "precision", "format",
  "source", "target", "origin", "destination", "channel", "topic",
  "payload", "metadata", "context", "options", "settings", "overrides",
  "width2", "offset", "limit", "page", "pageSize", "totalPages",
  "avatar", "banner", "thumbnail", "mimeType", "sizeBytes", "encoding",
  "latitude", "longitude", "altitude", "accuracy", "bearing", "speed",
];

const FIELD_TYPES = [
  "string", "number", "boolean", "Date", "string[]", "number[]",
  "Record<string, unknown>", "string | null", "number | undefined",
];

const BASE_TYPES = ["Auditable", "Identified", "Timestamped", "Taggable"];

const VERBS = [
  "load", "save", "fetch", "build", "render", "parse", "format", "resolve",
  "compute", "collect", "merge", "apply", "select", "reduce", "expand",
  "validate", "normalize", "serialize", "compare", "summarize", "project",
];

const NOUNS = [
  "Order", "Invoice", "Account", "Profile", "Session", "Report", "Ledger",
  "Widget", "Bundle", "Manifest", "Snapshot", "Digest", "Roster", "Payload",
  "Channel", "Segment", "Cohort", "Batch", "Route", "Policy", "Quota",
];

// Deliberately re-used across files so Design.DuplicateExportName has data.
const SHARED_EXPORT_NAMES = ["handler", "toDto", "fromDto", "defaults", "createClient"];

const LOCAL_NAMES = [
  "acc", "total", "cursor", "buf", "tmp", "seen", "out", "carry", "head",
  "chunk", "sum", "ratio", "count", "bucket", "slot", "entry", "row", "col",
  "delta", "scale", "window", "frame", "cell", "node", "leaf", "edge",
];

// ─── type shapes ───────────────────────────────────────────────────────────

/// The canonical field set for a family. Every family gets a marker field
/// named after its own id, which is what guarantees cross-family similarity
/// stays at zero: without it, two unrelated 5-field shapes drawn from a
/// 100-name vocabulary would occasionally collide by chance and the duplicate
/// rate would stop being a knob.
function familyFields(familyId, rng) {
  const count = int(rng, 5, 12);
  const fields = [{ name: `f${familyId}Key`, type: "string" }];
  const used = new Set([fields[0].name]);
  while (fields.length < count) {
    const name = pick(rng, FIELD_NAMES);
    if (used.has(name)) continue;
    used.add(name);
    fields.push({ name, type: pick(rng, FIELD_TYPES) });
  }
  return fields;
}

/// Derive a family member from the canonical field set.
///
/// The check flags a pair when |matching fields| / |union of field names| >= 0.8.
/// With n in 5..12 that makes `exact` and `add` always match, `retype`/`drop`
/// match (both give (n-1)/n), and `diverge` (two swaps, giving (n-2)/(n+2))
/// never match for n <= 12. So the mix produces near-duplicates on both sides
/// of the threshold, not just flagged ones.
function memberFields(base, mode, rng) {
  const fields = base.map((f) => ({ ...f }));
  switch (mode) {
    case "exact":
      return fields;
    case "add":
      fields.push({ name: `extra${int(rng, 0, 999)}`, type: pick(rng, FIELD_TYPES) });
      return fields;
    case "retype": {
      const i = int(rng, 1, fields.length - 1);
      const current = fields[i].type;
      fields[i].type = FIELD_TYPES.find((t) => t !== current);
      return fields;
    }
    case "drop":
      fields.splice(int(rng, 1, fields.length - 1), 1);
      return fields;
    case "diverge": {
      for (let k = 0; k < 2; k++) {
        fields[int(rng, 1, fields.length - 1)] = {
          name: `only${int(rng, 0, 9999)}`,
          type: pick(rng, FIELD_TYPES),
        };
      }
      return fields;
    }
    default:
      return fields;
  }
}

const MEMBER_MODES = ["exact", "add", "retype", "drop", "diverge"];

/// Build the flat list of shape assignments, one per shape that will be
/// emitted. Families are laid out contiguously then shuffled, so members of a
/// family end up scattered across the directory tree rather than adjacent.
function planTypeShapes(count, familyRate, rng) {
  const shapes = [];
  let familyId = 0;
  while (shapes.length < count) {
    const size = rng() < familyRate ? int(rng, 2, 4) : 1;
    const base = familyFields(familyId, rng);
    // ~12% of families extend a shared base. Members share it, since the
    // check only compares shapes whose (sorted) extends sets are identical.
    const extendsBase = rng() < 0.12 ? pick(rng, BASE_TYPES) : null;
    for (let m = 0; m < size && shapes.length < count; m++) {
      const mode = m === 0 ? "exact" : pick(rng, MEMBER_MODES);
      shapes.push({
        familyId,
        member: m,
        fields: memberFields(base, mode, rng),
        extendsBase,
        singleton: size === 1,
      });
    }
    familyId++;
  }
  return shuffle(rng, shapes);
}

function renderTypeShape(shape, name, rng) {
  const body = shape.fields
    .map((f) => `  ${f.name}${rng() < 0.15 ? "?" : ""}: ${f.type};`)
    .join("\n");
  if (shape.extendsBase) {
    return `export interface ${name} extends ${shape.extendsBase} {\n${body}\n}`;
  }
  // Mix declaration forms; the check handles both.
  return rng() < 0.75
    ? `export interface ${name} {\n${body}\n}`
    : `export type ${name} = {\n${body}\n};`;
}

// ─── duplicate code blocks ─────────────────────────────────────────────────

// Each template is exactly 6 statements (the check's default min_statements)
// and comfortably over the 80-char floor. `n` are local identifier names —
// varied per occurrence, and canonicalised away by the check — while `L` are
// numeric literals, which are hashed verbatim and therefore decide whether two
// occurrences collide.
const BLOCK_TEMPLATES = [
  (n, L) => `  const ${n[0]} = items.length;
  let ${n[1]} = ${L[0]};
  for (const ${n[2]} of items) {
    if (${n[2]}.score > ${L[1]}) {
      ${n[1]} += ${n[2]}.score;
    }
  }
  const ${n[3]} = ${n[1]} / Math.max(${n[0]}, 1);
  if (${n[3]} > ${L[2]}) {
    return Math.round(${n[3]});
  }
  return ${L[0]};`,

  (n, L) => `  const ${n[0]}: string[] = [];
  let ${n[1]} = ${L[0]};
  for (const ${n[2]} of items) {
    ${n[0]}.push(${n[2]}.label);
    ${n[1]} += ${n[2]}.label.length;
  }
  const ${n[3]} = ${n[0]}.join("-");
  if (${n[3]}.length > ${L[1]}) {
    return ${n[1]};
  }
  return ${L[2]};`,

  (n, L) => `  let ${n[0]} = ${L[0]};
  const ${n[1]} = items.filter((e) => e.score > ${L[1]});
  for (let ${n[2]} = 0; ${n[2]} < ${n[1]}.length; ${n[2]}++) {
    ${n[0]} = ${n[0]} + ${n[1]}[${n[2]}].score;
  }
  const ${n[3]} = ${n[1]}.length > 0 ? ${n[0]} / ${n[1]}.length : ${L[2]};
  const ${n[4]} = ${n[3]} * 2;
  try {
    return Math.trunc(${n[4]});
  } catch {
    return ${L[2]};
  }`,

  (n, L) => `  const ${n[0]} = new Map<string, number>();
  for (const ${n[1]} of items) {
    ${n[0]}.set(${n[1]}.label, (${n[0]}.get(${n[1]}.label) ?? ${L[0]}) + ${n[1]}.score);
  }
  let ${n[2]} = ${L[1]};
  ${n[0]}.forEach((value) => {
    ${n[2]} += value;
  });
  const ${n[3]} = ${n[0]}.size;
  switch (true) {
    case ${n[3]} > ${L[2]}:
      return Math.floor(${n[2]});
    default:
      return ${L[1]};
  }`,
];

function planBlocks(count, familyRate, rng) {
  const blocks = [];
  let key = 0;
  while (blocks.length < count) {
    const size = rng() < familyRate ? int(rng, 2, 4) : 1;
    const template = int(rng, 0, BLOCK_TEMPLATES.length - 1);
    // Literal triple is what makes two occurrences of the same template hash
    // identically; a per-family value keeps distinct families apart.
    const literals = [key % 7, 10 + (key % 31), 100 + (key % 53)];
    for (let m = 0; m < size && blocks.length < count; m++) {
      blocks.push({ template, literals, singleton: size === 1 });
    }
    key++;
  }
  return shuffle(rng, blocks);
}

function renderBlockFn(block, name, rng) {
  const names = shuffle(rng, [...LOCAL_NAMES]).slice(0, 5);
  const body = BLOCK_TEMPLATES[block.template](names, block.literals);
  return `export function ${name}(items: Array<{ score: number; label: string }>): number {\n${body}\n}`;
}

// ─── ordinary functions (length / complexity / nesting variety) ────────────

function renderPlainFn(name, rng) {
  const stmts = int(rng, 2, 9);
  const lines = [];
  const local = shuffle(rng, [...LOCAL_NAMES]).slice(0, 6);
  lines.push(`  let ${local[0]} = input.length;`);
  for (let i = 1; i < stmts; i++) {
    const roll = rng();
    if (roll < 0.3) {
      lines.push(`  if (${local[0]} > ${int(rng, 1, 40)}) {`);
      lines.push(`    ${local[0]} -= ${int(rng, 1, 5)};`);
      lines.push(`  }`);
    } else if (roll < 0.5) {
      lines.push(`  for (const ${local[1 + (i % 5)]} of input) {`);
      lines.push(`    if (${local[1 + (i % 5)]}.length > ${int(rng, 1, 9)}) {`);
      lines.push(`      ${local[0]} += ${local[1 + (i % 5)]}.length;`);
      lines.push(`    }`);
      lines.push(`  }`);
    } else if (roll < 0.65) {
      lines.push(`  ${local[0]} = ${local[0]} > 0 ? ${local[0]} - 1 : ${int(rng, 0, 3)};`);
    } else {
      lines.push(`  ${local[0]} += ${int(rng, 1, 20)};`);
    }
  }
  lines.push(`  return ${local[0]};`);
  return `export function ${name}(input: string[]): number {\n${lines.join("\n")}\n}`;
}

/// A long, deeply nested, branchy function. Emitted for a slice of files so
/// Refactor.LongAndComplex / nesting checks aren't trivially empty across
/// the whole corpus.
function renderComplexFn(name, rng) {
  const local = shuffle(rng, [...LOCAL_NAMES]).slice(0, 8);
  const lines = [`  let ${local[0]} = 0;`, `  const ${local[1]}: number[] = [];`];
  for (let i = 0; i < int(rng, 4, 9); i++) {
    lines.push(`  for (const ${local[2]} of rows) {`);
    lines.push(`    if (${local[2]}.score > ${int(rng, 1, 50)}) {`);
    lines.push(`      if (${local[2]}.label.length % 2 === 0) {`);
    lines.push(`        ${local[0]} += ${local[2]}.score;`);
    lines.push(`      } else if (${local[2]}.label.startsWith("${pick(rng, NOUNS)}")) {`);
    lines.push(`        ${local[1]}.push(${local[2]}.score);`);
    lines.push(`      } else {`);
    lines.push(`        ${local[0]} -= ${int(rng, 1, 9)};`);
    lines.push(`      }`);
    lines.push(`    }`);
    lines.push(`  }`);
  }
  lines.push(`  return ${local[0]} + ${local[1]}.length;`);
  return `export function ${name}(rows: Array<{ score: number; label: string }>): number {\n${lines.join("\n")}\n}`;
}

// ─── layout ────────────────────────────────────────────────────────────────

/// Plausible app tree: a small shared core that everything imports from, and
/// the bulk of files spread across feature directories. Not one flat dir —
/// discovery, path handling and the import graph all behave differently on a
/// tree with depth.
function planLayout(total, rng) {
  const typeModules = Math.max(3, Math.round(total * 0.02));
  const utilModules = Math.max(4, Math.round(total * 0.05));
  const featureFiles = Math.max(1, total - typeModules - utilModules);
  const featureCount = Math.max(1, Math.round(featureFiles / 12));

  const files = [];
  for (let i = 0; i < typeModules; i++) {
    files.push({ path: `src/shared/types/model${i}.ts`, role: "types" });
  }
  for (let i = 0; i < utilModules; i++) {
    files.push({ path: `src/shared/utils/util${i}.ts`, role: "utils" });
  }
  for (let i = 0; i < featureFiles; i++) {
    const feature = i % featureCount;
    const kind = pick(rng, ["component", "service", "store", "adapter", "view", "hooks"]);
    files.push({
      path: `src/features/f${feature}/${kind}${Math.floor(i / featureCount)}.ts`,
      role: "feature",
      feature,
    });
  }
  return files;
}

function relImport(fromPath, toPath) {
  const spec = posix.relative(posix.dirname(fromPath), toPath).replace(/\.ts$/, "");
  return spec.startsWith(".") ? spec : `./${spec}`;
}

// ─── generation ────────────────────────────────────────────────────────────

function generate(opts) {
  const rng = mulberry32(opts.seed);
  const files = planLayout(opts.files, rng);

  const typeFiles = files.filter((f) => f.role === "types");
  const utilFiles = files.filter((f) => f.role === "utils");

  // ~65% of files declare a type shape; ~30% carry a duplicate-candidate block.
  const shapeCount = Math.round(files.length * 0.65);
  const blockCount = Math.round(files.length * 0.3);
  const shapes = planTypeShapes(shapeCount, opts.typeFamilyRate, rng);
  const blocks = planBlocks(blockCount, opts.blockFamilyRate, rng);

  let shapeCursor = 0;
  let blockCursor = 0;
  const written = [];

  for (const [index, file] of files.entries()) {
    const parts = [];
    const imports = [];

    // Import graph. Shared modules are imported widely (fan-in); feature files
    // also pull from siblings, which is where the fan-out outliers come from.
    // Every import is referenced below — a corpus where most files carry a
    // trivially-unused import would drown the other checks in noise.
    let usesModelBase = false;
    const utilRefs = [];
    if (file.role === "feature") {
      const chosen = new Set();
      for (let i = 0, n = int(rng, 1, 4); i < n && utilFiles.length; i++) {
        chosen.add(utilFiles.indexOf(pick(rng, utilFiles)));
      }
      for (const idx of chosen) {
        imports.push(
          `import { util${idx}Value } from "${relImport(file.path, utilFiles[idx].path)}";`,
        );
        utilRefs.push(`util${idx}Value`);
      }
      if (typeFiles.length && rng() < 0.6) {
        const target = pick(rng, typeFiles).path;
        imports.push(`import type { ModelBase } from "${relImport(file.path, target)}";`);
        usesModelBase = true;
      }
      const siblings = files.filter(
        (f) => f.role === "feature" && f.feature === file.feature && f.path !== file.path,
      );
      if (siblings.length && rng() < 0.5) {
        const target = pick(rng, siblings).path;
        const siblingIndex = files.findIndex((f) => f.path === target);
        imports.push(
          `import { featureConst${siblingIndex} as siblingConst } from "${relImport(file.path, target)}";`,
        );
        utilRefs.push("siblingConst.length");
      }
    } else if (file.role === "utils" && typeFiles.length && rng() < 0.4) {
      const target = pick(rng, typeFiles).path;
      imports.push(`import type { ModelBase } from "${relImport(file.path, target)}";`);
      usesModelBase = true;
    }
    if (imports.length) parts.push(imports.join("\n"));

    if (file.role === "types") {
      parts.push(
        BASE_TYPES.map(
          (b) => `export interface ${b} {\n  ${b.toLowerCase()}Id: string;\n  revision: number;\n}`,
        ).join("\n\n"),
      );
      parts.push(`export interface ModelBase {\n  id: string;\n  kind: string;\n  revision: number;\n}`);
    }
    if (file.role === "utils") {
      const n = utilFiles.findIndex((f) => f.path === file.path);
      parts.push(`export const util${n}Value = ${int(rng, 1, 999)};`);
    }
    if (file.role === "feature") {
      parts.push(`export const featureConst${index} = "${pick(rng, NOUNS).toLowerCase()}-${index}";`);
    }
    if (utilRefs.length) {
      parts.push(`export const importedTotal${index} = ${utilRefs.join(" + ")};`);
    }
    if (usesModelBase) {
      parts.push(
        `export function identify${index}(base: ModelBase): string {\n  return \`\${base.kind}:\${base.id}\`;\n}`,
      );
    }

    if (shapeCursor < shapes.length && rng() < 0.8) {
      const shape = shapes[shapeCursor++];
      const name = `${pick(rng, NOUNS)}${pick(rng, ["Dto", "Model", "Record", "Shape", "Props", "Config"])}${index}`;
      parts.push(renderTypeShape(shape, name, rng));
      // A second shape in the same file, occasionally — real files often
      // declare a request/response pair.
      if (shapeCursor < shapes.length && rng() < 0.25) {
        const extra = shapes[shapeCursor++];
        parts.push(renderTypeShape(extra, `${name}Input`, rng));
      }
    }

    if (blockCursor < blocks.length && rng() < 0.6) {
      const block = blocks[blockCursor++];
      parts.push(renderBlockFn(block, `${pick(rng, VERBS)}${pick(rng, NOUNS)}${index}`, rng));
    }

    // Some export names are drawn from a small shared pool so
    // Design.DuplicateExportName has genuine cross-file collisions.
    for (let k = 0, n = int(rng, 1, 3); k < n; k++) {
      const fnName =
        k === 0 && rng() < 0.12
          ? pick(rng, SHARED_EXPORT_NAMES)
          : `${pick(rng, VERBS)}${pick(rng, NOUNS)}${index}_${k}`;
      parts.push(renderPlainFn(fnName, rng));
    }

    if (rng() < 0.12) {
      parts.push(renderComplexFn(`analyze${pick(rng, NOUNS)}${index}`, rng));
    }

    const source = `${parts.join("\n\n")}\n`;
    const abs = join(opts.out, file.path);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, source, "utf8");
    written.push({ path: file.path, bytes: Buffer.byteLength(source) });
  }

  return { written, shapeCount: shapeCursor, blockCount: blockCursor };
}

function writeProjectFiles(out) {
  writeFileSync(
    join(out, "package.json"),
    `${JSON.stringify(
      { name: "cofferdam-synthetic-bench", version: "0.0.0", private: true, type: "module" },
      null,
      2,
    )}\n`,
    "utf8",
  );
  writeFileSync(
    join(out, "tsconfig.json"),
    `${JSON.stringify(
      {
        compilerOptions: {
          target: "ES2022",
          module: "ESNext",
          moduleResolution: "Bundler",
          strict: true,
          noEmit: true,
          skipLibCheck: true,
        },
        include: ["src"],
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
}

// ─── main ──────────────────────────────────────────────────────────────────

const opts = parseArgs(argv.slice(2));
opts.out = resolve(opts.out);

if (opts.clean) rmSync(opts.out, { recursive: true, force: true });
mkdirSync(opts.out, { recursive: true });

const result = generate(opts);
writeProjectFiles(opts.out);

const totalBytes = result.written.reduce((a, f) => a + f.bytes, 0);
stdout.write(
  `generated ${result.written.length} files in ${opts.out}\n` +
    `  seed=${opts.seed} type-family-rate=${opts.typeFamilyRate} block-family-rate=${opts.blockFamilyRate}\n` +
    `  ${result.shapeCount} type shapes, ${result.blockCount} duplicate-candidate blocks\n` +
    `  ${(totalBytes / 1024 / 1024).toFixed(1)} MiB total, ${Math.round(totalBytes / result.written.length)} bytes/file avg\n`,
);
