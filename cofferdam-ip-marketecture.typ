// Cofferdam — IP & Marketecture deck
// Self-contained (no @preview imports) so it compiles offline.
// Render:  typst compile cofferdam-ip-marketecture.typ
#set document(title: "Cofferdam — IP & Marketecture", author: "Thomas Dickson")

#let accent  = rgb("#0e6e7a")
#let accent2 = rgb("#0a4f58")
#let ink     = rgb("#16242e")
#let muted   = rgb("#5d6f7a")
#let panelbg = rgb("#eef4f5")
#let done    = rgb("#2f7d33")
#let flight  = rgb("#b06a00")
#let todo    = rgb("#8a9aa3")

#set text(font: ("Segoe UI", "Arial"), size: 15pt, fill: ink)
#set par(leading: 0.6em)

// status chip:  d = done, f = in flight, o = not started
#let S(k) = (
  if k == "d" { text(fill: done, weight: "bold")[●] }
  else if k == "f" { text(fill: flight, weight: "bold")[◐] }
  else { text(fill: todo)[○] }
)

#let legend = text(size: 11pt, fill: muted)[
  #S("d") shipped #h(10pt) #S("f") in flight #h(10pt) #S("o") not started
]

#let panel(title, body) = rect(
  width: 100%, fill: panelbg, stroke: 0.6pt + accent.lighten(35%),
  radius: 6pt, inset: 11pt,
)[
  #text(fill: accent2, weight: "bold", size: 13pt)[#title]
  #v(3pt)
  #set text(size: 12.5pt)
  #body
]

#let card(title, body) = rect(
  width: 100%, fill: white, stroke: (left: 3pt + accent, rest: 0.5pt + muted.lighten(45%)),
  radius: 4pt, inset: 11pt,
)[
  #text(weight: "bold", size: 14pt, fill: accent2)[#title]
  #v(3pt)
  #set text(size: 12pt, fill: muted)
  #body
]

// content slide
#let slide(title, body) = page(paper: "presentation-16-9", margin: 0pt, fill: white)[
  #block(width: 100%, fill: accent, inset: (x: 1.0cm, y: 0.46cm))[
    #text(fill: white, weight: "bold", size: 21pt)[#title]
  ]
  #block(width: 100%, inset: (x: 1.0cm, top: 0.5cm, bottom: 0.5cm))[#body]
  #place(bottom + right, dx: -0.5cm, dy: -0.35cm, text(size: 9pt, fill: muted)[cofferdam · v0.3.5])
]

// ───────────────────────── title ─────────────────────────
#page(paper: "presentation-16-9", margin: 0pt, fill: accent)[
  #block(width: 100%, height: 100%, inset: (x: 1.4cm, y: 1.4cm))[
    #v(1fr)
    #text(fill: white, weight: "bold", size: 46pt)[Cofferdam]
    #v(2pt)
    #text(fill: white.darken(4%), size: 24pt)[Code management for the AI age]
    #v(14pt)
    #text(fill: rgb("#cfe6e9"), size: 17pt)[
      Standardise how AI writes code — with the Credo-style linter rolled up beneath it.
    ]
    #v(1fr)
    #text(fill: rgb("#bcdde1"), size: 13pt)[IP & Marketecture · v0.3.5 · 193 / 200 beads closed]
  ]
]

// ───────────────────────── thesis ─────────────────────────
#slide("The thesis")[
  #v(0.5cm)
  #text(size: 26pt, weight: "bold", fill: accent2)[
    Code is a commodity that agents now generate fast.
  ]
  #v(10pt)
  #text(size: 21pt)[
    Cofferdam is how a team pins down what *“good”* means #emph[once] — and makes every
    author, human or AI, comply with it #emph[without reading the whole codebase].
  ]
  #v(16pt)
  #grid(columns: (1fr, 1fr, 1fr), gutter: 14pt,
    panel("Declare once")[The architecture lives in one file: `cofferdam.invariants.toml`.],
    panel("Comply before the edit")[`advise` pushes the rules to the agent #emph[ahead] of the change.],
    panel("Roll up the linter")[The Credo five-category checks are the floor, not the product.],
  )
]

// ───────────────────────── positioning ─────────────────────────
#slide("Positioning: from linter to code-management")[
  #v(0.3cm)
  #grid(columns: (1fr, 1fr), gutter: 18pt,
    card("Old framing — “Credo for TypeScript”")[
      A TS static analyzer with five prioritized categories. \
      Competes with ESLint on rule count. \
      Reacts #emph[after] code is written. \
      #v(4pt) #text(fill: todo)[Comparison set: ESLint, tslint — a crowded race to the bottom.]
    ],
    card("New framing — “code management for the AI age”")[
      The substrate for an organisation's code standards. \
      Agents comply against the spec #emph[before] they write. \
      Linting is one rolled-up layer. \
      #v(4pt) #text(fill: accent2, weight: "bold")[Comparison set: Credo, Semgrep, fitness functions — and the AI toolchain.]
    ],
  )
  #v(12pt)
  #align(center)[#text(size: 17pt, fill: accent2, weight: "bold")[
    “Cofferdam isn't a linter, it's a code management tool.” — design-principles.md
  ]]
]

// ───────────────────────── IP claims ─────────────────────────
#slide("The IP — four defensible claims")[
  #v(0.25cm)
  #grid(columns: (1fr, 1fr), gutter: 12pt,
    card("1 · Spec as shared truth")[
      Architecture declared once, read by many checks #emph[and] by agents.
      One contract, N enforcement points — vs a linter's per-rule config that drifts.
    ],
    card("2 · Agents comply before they write")[
      `advise <file>` pushes applicable rules ahead of the edit; `advise --diff` pre-flights a
      change (`would_fire` / `would_clear`). Every other linter only reacts #emph[after].
    ],
    card("3 · Cross-file architectural reasoning")[
      Canonical graph + typed corpus → layer violations, import cycles, orphan / dead exports,
      frozen boundaries, scripted invariants. The leapfrog over Credo / ESLint.
    ],
    card("4 · Polylingual substrate")[
      Rules run on a language-agnostic graph; languages plug in via adapters (Rust adapter is
      the live proof). Opinions outlive the language.
    ],
  )
]

// ───────────────────────── moat ─────────────────────────
#slide("The moat: the standardisation flywheel")[
  #v(0.6cm)
  #align(center)[
    #grid(columns: (auto, auto, auto, auto, auto), gutter: 10pt, align: horizon,
      panel("Declare")[Team encodes its architecture in the spec.],
      text(size: 26pt, fill: accent)[→],
      panel("Comply")[Agents `advise` against it on every edit.],
      text(size: 26pt, fill: accent)[→],
      panel("Compound")[More rules — Rust or TS-plugin — read the same spec.],
    )
  ]
  #v(18pt)
  #text(size: 19pt)[
    Once a team's architecture #emph[and] its agents both depend on the spec, it becomes the
    canonical contract for all code production. Switching cost climbs; value compounds with
    every rule added.
  ]
  #v(10pt)
  #text(size: 14pt, fill: muted)[
    The operational discipline — stable IDs, baseline adoption, severity/priority split,
    suppression-as-metadata, a single clean `--robot` channel — is what makes the opinions
    #emph[trustworthy] for CI and agents alike.
  ]
]

// ───────────────────────── marketecture I ─────────────────────────
#slide("Marketecture I — the loop")[
  #place(top + right, dy: -0.95cm, legend)
  #v(0.15cm)
  #panel("THE SPEC · cofferdam.invariants.toml — declared once, read by many")[
    #S("d") layers + allow #h(8pt) #S("d") public_api #h(8pt) #S("d") boundaries (frozen)
    #h(8pt) #S("d") invariants (forbid / require) #h(8pt) #S("d") scripted predicate DSL
    #h(8pt) #S("d") schema versioning
  ]
  #align(center)[#text(size: 13pt, fill: muted)[↓ read by three audiences ↓]]
  #v(4pt)
  #grid(columns: (1fr, 1.25fr, 1fr), gutter: 12pt,
    panel("Humans")[
      One file #emph[is] the architecture — \ not folklore. An architect reads \ it on day one.
    ],
    panel("Agents · the AI-age surface")[
      #S("d") `advise <file>` — rules before the edit \
      #S("d") `advise --diff` — pre-flight a change \
      #S("d") `--robot` JSON — clean data channel \
      #S("d") type-aware checks (ts-morph) \
      #S("o") `cofferdam-mcp` — native MCP tool (cd-9r3)
    ],
    panel("The engine")[
      #S("d") discovery + parse \
      #S("d") 3-phase Check contract \
      #S("d") corpus + finalize \
      #S("d") incremental cache
    ],
  )
]

// ───────────────────────── marketecture II ─────────────────────────
#slide("Marketecture II — the stack")[
  #place(top + right, dy: -0.95cm, legend)
  #v(0.15cm)
  #panel("FOUR LEVERAGE LAYERS · narrow → wide")[
    #S("d") built-in checks #h(6pt) → #h(6pt) #S("d") per-check options + per-glob `[[overrides]]`
    (cd-m5tu, 0.3.5) #h(6pt) → #h(6pt) #S("d") invariants spec #emph[(the centerpiece)]
    #h(6pt) → #h(6pt) #S("d") plugin SDK `@cofferdam/check-sdk` (cross-file corpus)
  ]
  #v(8pt)
  #grid(columns: (1fr, 1fr), gutter: 12pt,
    panel("Polylingual substrate · the frontier")[
      #S("d") canonical graph schema #h(6pt) #S("d") Rust adapter proof \
      #S("d") Span→Location refactor #h(6pt) #S("d") LSP server over stdio \
      #S("o") adapter contract: any artifact → graph (cd-9hp.10) \
      #S("o") Location variant rendering + TS SDK (cd-0gwd)
    ],
    panel("Distribution")[
      #S("d") GH binary releases + npm wrapper \
      #S("d") `cofferdam-check` GitHub Action \
      #S("d") SARIF → GitHub Code Scanning \
      #S("d") baseline · `--since` · severity gate \
      #S("d") VitePress docs + generated catalog
    ],
  )
]

// ───────────────────────── the floor ─────────────────────────
#slide("The Credo floor — 27 checks, rolled up as one layer")[
  #v(0.2cm)
  #grid(columns: (1fr, 1fr, 1fr, 1fr, 1fr), gutter: 9pt,
    panel("Warning · 6")[#set text(size: 11pt); TripleEquals \ NoConsoleLog \ NoDebugger \ NoEval \ UnusedImport \ UnusedNullCheck],
    panel("Refactor · 8")[#set text(size: 11pt); CyclomaticComplexity \ CognitiveComplexity \ DuplicateBlock \ UnusedVariable \ PreferOptionalChain \ PreferNullishCoalescing \ DeadExport \ LongAndComplex],
    panel("Design · 8")[#set text(size: 11pt); LayerViolation \ OrphanExport \ BoundaryFrozen \ InvariantViolation \ ImportCycle \ DuplicateExportName \ MaxParameters \ ScriptedInvariant],
    panel("Readability · 2")[#set text(size: 11pt); MaxLineLength \ MaxFunctionLength],
    panel("Consistency · 3")[#set text(size: 11pt); QuoteStyle \ UnusedSuppression \ BroadSuppression],
  )
  #v(12pt)
  #text(size: 15pt, fill: muted)[
    Five categories, priority-sorted within each — the Credo discipline. Configurable by option,
    by glob, or disabled entirely. This is the #emph[floor], not the product.
  ]
]

// ───────────────────────── status ─────────────────────────
#slide("Status — the spine is done; the frontier remains")[
  #v(0.2cm)
  #grid(columns: (1fr, 1fr), gutter: 18pt,
    card("Done — 193 / 200 beads")[
      #set text(size: 12pt, fill: ink)
      #S("d") AI surface: `advise`, `advise --diff`, `--robot`, type-aware \
      #S("d") Spec: invariants + predicate DSL + schema versioning + e2e fixtures \
      #S("d") Architecture epic cd-9hp: 11 / 12 \
      #S("d") Plugin SDK + cross-file corpus \
      #S("d") Canonical graph · Rust adapter · LSP \
      #S("d") Per-glob `[[overrides]]` (shipped 0.3.5) \
      #S("d") 27 checks · SARIF + 3 formatters · full distribution
    ],
    card("Left to do — 5 beads, none blocking")[
      #set text(size: 12pt, fill: ink)
      #S("o") *cd-9hp.10* (P3) adapter contract → graph \ #h(14pt) #text(size:10.5pt, fill: muted)[the last unredeemed architecture pledge] \
      #S("o") *cd-0gwd* (P3) Location rendering + TS SDK exposure \
      #S("o") *cd-9r3* (P4) `cofferdam-mcp` native MCP server \ #h(14pt) #text(size:10.5pt, fill: muted)[finishes the AI-context-substrate story] \
      #S("o") *cd-i255* (P4) lint Typst packages (dogfood polylingual) \
      #S("o") *cd-mhks* (P3) flaky timing bench tests
    ],
  )
]

// ───────────────────────── closing ─────────────────────────
#page(paper: "presentation-16-9", margin: 0pt, fill: accent)[
  #block(width: 100%, height: 100%, inset: (x: 1.4cm, y: 1.4cm))[
    #v(1fr)
    #text(fill: white, weight: "bold", size: 30pt)[Where it's heading]
    #v(12pt)
    #text(fill: rgb("#e7f3f4"), size: 20pt)[
      The linter is shipped. The spec + agent-advisory loop is shipped. \
      What remains turns cofferdam from “Credo-for-TS with an AI surface” into a
      #emph[polylingual standardisation substrate]: the adapter contract makes the rules
      language-agnostic; the MCP server makes the agent surface a first-class tool.
    ]
    #v(1fr)
    #text(fill: rgb("#bcdde1"), size: 14pt)[github.com/TAJD/cofferdam · v0.3.5]
  ]
]
