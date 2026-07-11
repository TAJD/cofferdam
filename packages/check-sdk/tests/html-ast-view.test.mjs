// CD-84: HTML AstView surface — Element/Attribute/Text/Comment/Doctype
// walkable via `file.ast` against `.html` files. Wires below are
// hand-built to match what `crates/cofferdam-cli/src/html_wire.rs`
// emits, mirroring the CD-80 JSXElement test's pattern in
// `plugin-host.test.mjs`.
//
// Run: node --test tests/html-ast-view.test.mjs (after `npm run build`)

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const ROOT = dirname(fileURLToPath(import.meta.url));
const PKG = resolve(ROOT, "..");

async function loadSdk() {
  return import(`file://${PKG.replace(/\\/g, "/")}/dist/index.js`);
}

const S = { line: 1, column: 1, start_byte: 0, end_byte: 1 };

test("findAll(Element) finds <img> with no alt, filterable by tag name and attributes", async () => {
  const { defineCheck, Category, runPlugin } = await loadSdk();

  const check = defineCheck({
    id: "ImgMissingAlt",
    category: Category.Warning,
    basePriority: 10,
    explanation: "An <img> element has no `alt` attribute.",
    files: { extensions: ["html"] },
    run(file, ctx) {
      if (!file.ast) return;
      for (const el of file.ast.findAll("Element")) {
        if (el.tagName !== "img") continue;
        const hasAlt = el.attributes.some((a) => a.name === "alt");
        if (!hasAlt) {
          ctx.report({ message: `<img> at byte ${el.span.start_byte} is missing alt`, span: el.span });
        }
      }
    },
  });

  // Wire matching what html_wire.rs emits for:
  //   <html><body><img src="cat.png"><img src="dog.png" alt="a dog"></body></html>
  const ast = {
    rootIdx: 0,
    nodes: [
      // 0: Document
      { kind: "Document", span: S, firstChild: 1, nextSibling: -1 },
      // 1: <html> element
      {
        kind: "Element",
        span: S,
        firstChild: 2,
        nextSibling: -1,
        tagName: "html",
        selfClosing: false,
        attributeIdxs: [],
      },
      // 2: <body> element
      {
        kind: "Element",
        span: S,
        firstChild: 3,
        nextSibling: -1,
        tagName: "body",
        selfClosing: false,
        attributeIdxs: [],
      },
      // 3: <img src="cat.png"> — no alt
      {
        kind: "Element",
        span: { line: 1, column: 1, start_byte: 12, end_byte: 32 },
        firstChild: 4,
        nextSibling: 5,
        tagName: "img",
        selfClosing: false,
        attributeIdxs: [4],
      },
      // 4: src="cat.png"
      { kind: "Attribute", span: S, firstChild: -1, nextSibling: -1, name: "src", value: "cat.png" },
      // 5: <img src="dog.png" alt="a dog"> — has alt
      {
        kind: "Element",
        span: { line: 1, column: 1, start_byte: 32, end_byte: 64 },
        firstChild: 6,
        nextSibling: -1,
        tagName: "img",
        selfClosing: false,
        attributeIdxs: [6, 7],
      },
      // 6: src="dog.png"
      { kind: "Attribute", span: S, firstChild: -1, nextSibling: 7, name: "src", value: "dog.png" },
      // 7: alt="a dog"
      { kind: "Attribute", span: S, firstChild: -1, nextSibling: -1, name: "alt", value: "a dog" },
    ],
  };

  const reports = runPlugin(check, {
    path: "page.html",
    text: '<html><body><img src="cat.png"><img src="dog.png" alt="a dog"></body></html>',
    lineViews: [],
    ast,
  });

  assert.equal(reports.length, 1, "only the alt-less <img> is flagged");
  assert.equal(reports[0].startByte, 12);
});

test("Document.children and Element.children expose Text content (e.g. <title> contents)", async () => {
  const { defineCheck, Category, runPlugin } = await loadSdk();

  const check = defineCheck({
    id: "TitleReader",
    category: Category.Warning,
    basePriority: 0,
    explanation: "Reports the <title> element's text content.",
    run(file, ctx) {
      if (!file.ast) return;
      for (const el of file.ast.findAll("Element")) {
        if (el.tagName !== "title") continue;
        const text = el.children.filter((c) => c.kind === "Text").map((c) => c.text).join("");
        ctx.report({ message: `title="${text}"`, span: el.span });
      }
    },
  });

  // Wire for: <html><head><title>My Page</title></head></html>
  const ast = {
    rootIdx: 0,
    nodes: [
      { kind: "Document", span: S, firstChild: 1, nextSibling: -1 },
      { kind: "Element", span: S, firstChild: 2, nextSibling: -1, tagName: "html", selfClosing: false, attributeIdxs: [] },
      { kind: "Element", span: S, firstChild: 3, nextSibling: -1, tagName: "head", selfClosing: false, attributeIdxs: [] },
      { kind: "Element", span: S, firstChild: 4, nextSibling: -1, tagName: "title", selfClosing: false, attributeIdxs: [] },
      { kind: "Text", span: S, firstChild: -1, nextSibling: -1, text: "My Page" },
    ],
  };

  const reports = runPlugin(check, {
    path: "page.html",
    text: "<html><head><title>My Page</title></head></html>",
    lineViews: [],
    ast,
  });

  assert.equal(reports.length, 1);
  assert.equal(reports[0].message, 'title="My Page"');
});

// CD-84 acceptance: duplicate <title> text across files, via
// ctx.corpus + finalize — zero engine changes, same cross-file pattern
// as examples-plugins/duplicate-class/src/index.ts.
test("duplicate <title> text across two files is caught via ctx.corpus + finalize", async () => {
  const { defineCheck, Category, runPlugin, runPluginFinalize, PluginCorpusStore } = await loadSdk();

  const SLOT = "titles";
  const check = defineCheck({
    id: "DuplicateTitle",
    category: Category.Warning,
    basePriority: 10,
    explanation: "The <title> text is duplicated across more than one HTML file.",
    files: { extensions: ["html"] },
    run(file, ctx) {
      if (!file.ast) return;
      for (const el of file.ast.findAll("Element")) {
        if (el.tagName !== "title") continue;
        const text = el.children
          .filter((c) => c.kind === "Text")
          .map((c) => c.text)
          .join("")
          .trim();
        if (!text) continue;
        ctx.corpus.append(SLOT, { file: file.path, text, span: el.span });
      }
    },
    finalize(ctx) {
      const all = ctx.corpus.read(SLOT) ?? [];
      const byText = new Map();
      for (const rec of all) {
        const list = byText.get(rec.text);
        if (list) list.push(rec);
        else byText.set(rec.text, [rec]);
      }
      for (const [text, recs] of byText) {
        const files = new Set(recs.map((r) => r.file));
        if (files.size < 2) continue;
        const [primary, ...others] = recs;
        ctx.report({
          file: primary.file,
          message: `<title>"${text}" is duplicated across ${files.size} files.`,
          span: primary.span,
          related: others.map((r) => ({ file: r.file, span: r.span })),
        });
      }
    },
  });

  function titleWire(text) {
    return {
      rootIdx: 0,
      nodes: [
        { kind: "Document", span: S, firstChild: 1, nextSibling: -1 },
        { kind: "Element", span: S, firstChild: 2, nextSibling: -1, tagName: "title", selfClosing: false, attributeIdxs: [] },
        { kind: "Text", span: S, firstChild: -1, nextSibling: -1, text },
      ],
    };
  }

  const corpus = new PluginCorpusStore();
  const reportsA = runPlugin(
    check,
    { path: "a.html", text: "<title>Welcome</title>", lineViews: [], ast: titleWire("Welcome") },
    corpus,
  );
  const reportsB = runPlugin(
    check,
    { path: "b.html", text: "<title>Welcome</title>", lineViews: [], ast: titleWire("Welcome") },
    corpus,
  );
  const reportsC = runPlugin(
    check,
    { path: "c.html", text: "<title>Unique Page</title>", lineViews: [], ast: titleWire("Unique Page") },
    corpus,
  );
  assert.equal(reportsA.length, 0, "no per-file findings from run()");
  assert.equal(reportsB.length, 0);
  assert.equal(reportsC.length, 0);

  const finalReports = runPluginFinalize(check, corpus);
  assert.equal(finalReports.length, 1, "exactly one duplicate group (Welcome), Unique Page is not duplicated");
  const [r] = finalReports;
  assert.equal(r.checkId, "DuplicateTitle");
  assert.equal(r.file, "a.html");
  assert.equal(r.related.length, 1);
  assert.equal(r.related[0].file, "b.html");
  assert.match(r.message, /"Welcome" is duplicated across 2 files/);
});
