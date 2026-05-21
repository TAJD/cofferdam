#!/usr/bin/env node
// scripts/check-vitepress.mjs — VitePress markdown sanity check.
//
// The docs site (`docs/`) builds with VitePress, which routes
// markdown through Vue's template compiler. That pipeline has known
// rough edges that don't trip a normal markdown reader:
//
//   1. `<Capital...>` outside a fenced code block looks like an
//      HTML/Vue tag opening. Vue tries to find a matching `</Capital>`
//      and crashes with "Element is missing end tag". Real cases
//      we hit: `Vec<T>` in a markdown table cell (cd-rk1), TS
//      generics in inline narration.
//
//   2. `{{ ... }}` is Vue interpolation. The compiler greedily
//      extracts these even *inside inline backtick code spans*
//      (`<code>{{</code>` rendering). Real case: documenting DSL
//      escape syntax with literal `{{`.
//
//   3. Relative paths in markdown links that escape `docs/` look fine
//      to a markdown reader but VitePress' dead-link checker rejects
//      them — the deployed site has no file outside docs/.
//
// This script catches the three patterns. It runs in well under
// 100 ms (no docs build, no Vue), making it cheap enough for
// pre-commit. The full `pnpm docs:build` remains the authoritative
// gate in CI (`.github/workflows/docs.yml`).
//
// Fix recipes when this fires:
//   - Wrap `<Capital>` in inline backticks: `Vec<T>` → \`Vec<T>\`.
//     Markdown-it escapes `<` inside `<code>` to `&lt;`, so Vue
//     never sees a tag.
//   - For `{{` / `}}` that must appear in inline-code styling, drop
//     the backticks and use raw HTML with entities:
//       \`{{\`           →   <code>&#123;&#123;</code>
//     Entity-encoded `{{` survives Vue's interpolation scanner.
//   - For links to repo-root files (README.md, CLAUDE.md, etc.),
//     use the GitHub URL instead of `../foo.md`.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve, dirname } from "node:path";

const DOCS_DIR = resolve(import.meta.dirname, "..", "docs");
const REPO_ROOT = resolve(import.meta.dirname, "..");

// Directories under docs/ we don't scan: third-party deps, build
// output, and the generated check catalog (gen-docs owns those).
const SKIP_DIRS = new Set(["node_modules", ".vitepress", "checks"]);

/** Recursively gather *.md files under `dir`, skipping SKIP_DIRS. */
function gatherMarkdown(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      if (SKIP_DIRS.has(entry)) continue;
      out.push(...gatherMarkdown(full));
    } else if (entry.endsWith(".md")) {
      out.push(full);
    }
  }
  return out;
}

/**
 * Scan one markdown file. Returns an array of {line, column, rule,
 * message} violations. Tracks fenced-code-block state line-by-line
 * (toggled by ``` at start of line) and skips content inside those.
 */
function scanFile(path) {
  const raw = readFileSync(path, "utf8");
  // HTML comments are passed through markdown-it verbatim and stripped
  // by Vue's template parser — content inside them never reaches the
  // interpolation scanner. Drop them before line-splitting so rule
  // checks don't false-positive on examples written inside `<!-- -->`.
  // Newlines inside comments are preserved as blanks to keep line
  // numbers stable for the surviving content.
  const text = raw.replace(/<!--[\s\S]*?-->/g, (m) =>
    m.replace(/[^\n]/g, " "),
  );
  const lines = text.split(/\r?\n/);
  const violations = [];
  let inFence = false;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const lineNum = i + 1;

    // Toggle fenced-code state on lines starting with ``` (with
    // optional language tag). Inside the fence, none of the rules
    // below apply — fenced code is wrapped in <pre><code> which Vue
    // leaves alone.
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;

    // Rule 1: `{{` / `}}` outside fenced blocks. Vue interpolation
    // scanner picks these up even inside inline-backtick `<code>`
    // spans. The fix is to drop the backticks and use raw HTML with
    // entity escapes: `<code>&#123;&#123;</code>`.
    const doubleOpen = line.indexOf("{{");
    if (doubleOpen !== -1) {
      violations.push({
        line: lineNum,
        column: doubleOpen + 1,
        rule: "vue-interpolation",
        message:
          "literal `{{` triggers Vue interpolation. Use `<code>&#123;&#123;</code>` for literal double-brace pairs (entities bypass Vue's scanner).",
      });
    }
    const doubleClose = line.indexOf("}}");
    if (doubleClose !== -1) {
      violations.push({
        line: lineNum,
        column: doubleClose + 1,
        rule: "vue-interpolation",
        message:
          "literal `}}` triggers Vue interpolation. Use `<code>&#125;&#125;</code>` for literal double-brace pairs.",
      });
    }

    // Rule 2: `<Capital...>` patterns OUTSIDE inline backticks. Vue
    // treats anything `<X...>` as a tag and demands `</X>` to close
    // it. TS generics (`Vec<T>`, `Map<string, ClassDecl[]>`),
    // generic-flavoured type names (`Set<number>`,
    // `IterableIterator<LineView>`), and stray component-shaped
    // refs all trip it.
    //
    // To avoid false positives on prose, we look only at angle-bracket
    // pairs that are followed by `(`, `[`, `,`, `>`, whitespace, or
    // end-of-bracket-chain — i.e. they look like TS generic positions.
    //
    // Inline-backticked content (`\`Vec<T>\``) is safe: markdown-it
    // emits `<code>Vec&lt;T&gt;</code>`. We strip those before
    // checking.
    const stripped = stripInlineCode(line);
    const tagRegex = /<([A-Z][A-Za-z0-9_]*)\b[^<>]*>/g;
    let m;
    while ((m = tagRegex.exec(stripped)) !== null) {
      const tagName = m[1];
      // Heuristic whitelist: HTML elements we genuinely write in
      // raw form. Add to this when a real legitimate case appears.
      if (["Vue"].includes(tagName)) continue;
      // Self-closing tags `<X/>` are fine.
      if (m[0].endsWith("/>")) continue;
      // Tag pair on the same line — `<Foo>text</Foo>` — is also fine.
      const closeIdx = stripped.indexOf(`</${tagName}>`, m.index);
      if (closeIdx !== -1) continue;
      violations.push({
        line: lineNum,
        column: m.index + 1,
        rule: "vue-tag",
        message: `bare \`<${tagName}>\` outside backticks/fence is read as an HTML tag by Vue. Wrap the snippet in inline backticks (e.g. \`Vec<T>\`) or move into a fenced code block.`,
      });
    }

    // Rule 3: Markdown links of the form `(../X)` or `(../X.md)`
    // that resolve outside docs/. VitePress' dead-link check rejects
    // these — the deployed site has no file outside docs/. Use a
    // GitHub URL instead.
    //
    // Match the link target inside parens. Skip non-relative paths
    // (`https://`, `mailto:`, `#anchor`, absolute paths starting
    // with `/`).
    const linkRegex = /\]\(([^)]+)\)/g;
    let lm;
    while ((lm = linkRegex.exec(line)) !== null) {
      const target = lm[1].split("#")[0].split(" ")[0];
      if (!target.startsWith("../")) continue;
      if (/^[a-z]+:/.test(target)) continue;
      const fileDir = dirname(path);
      const resolved = resolve(fileDir, target);
      const docsRel = relative(DOCS_DIR, resolved);
      if (docsRel.startsWith("..") || docsRel === "" || docsRel.startsWith("../")) {
        violations.push({
          line: lineNum,
          column: lm.index + 2,
          rule: "external-relative-link",
          message: `link \`${target}\` resolves outside \`docs/\` (to \`${relative(REPO_ROOT, resolved)}\`). VitePress' dead-link check rejects these — use the GitHub URL instead, e.g. https://github.com/TAJD/cofferdam/blob/main/${relative(REPO_ROOT, resolved).replace(/\\/g, "/")}.`,
        });
      }
    }
  }

  return violations;
}

/**
 * Replace inline-backtick code spans with placeholder text of equal
 * length so position offsets stay aligned for the tag-pattern check.
 * Matches `\`...\`` (single backticks, single-line).
 */
function stripInlineCode(line) {
  return line.replace(/`[^`]*`/g, (m) => " ".repeat(m.length));
}

function main() {
  const files = gatherMarkdown(DOCS_DIR);
  let totalViolations = 0;
  for (const file of files) {
    const violations = scanFile(file);
    if (violations.length === 0) continue;
    totalViolations += violations.length;
    const rel = relative(REPO_ROOT, file).replace(/\\/g, "/");
    for (const v of violations) {
      console.error(`${rel}:${v.line}:${v.column}  [${v.rule}] ${v.message}`);
    }
  }
  if (totalViolations === 0) {
    console.log(`vitepress-check: ${files.length} files scanned, no issues.`);
    process.exit(0);
  } else {
    console.error(
      `\nvitepress-check: ${totalViolations} issue(s) in ${files.length} files. ` +
        `These would fail \`pnpm docs:build\` in CI.`,
    );
    process.exit(1);
  }
}

main();
