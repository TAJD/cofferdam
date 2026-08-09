# Consistency.SpellingDialect

Tallies British against American spellings across the project, then flags the
minority. A codebase that writes "behaviour" in 40 places and "behavior" in
three does not have two conventions; it has one convention and three outliers.

## What it reads

Comments, doc comments and prose-shaped string literals. Nothing else.

Identifiers are never scanned, and that is the load-bearing constraint rather
than an oversight. `normalize`, `serialize`, `initialize` and CSS `color` are
API surface: renaming a function to satisfy a prose convention would break
callers to fix nothing. A string literal counts as prose only if it contains a
space, which keeps import specifiers, object keys and check ids out of the
tally for the same reason.

The corpus stops at TypeScript. Markdown prose is not read, because a check
declares one language and this one declares TypeScript; CD-316 tracks lifting
that. A project whose documentation and code disagree will see only the code
half of the disagreement.

## Which dialect

By default, whichever the project already uses. The check ships no opinion —
deciding *which* dialect a team should write is a different and much harder
question than noticing the team has not decided, and only the second is
something a static analyser can answer.

Three things follow from that. The check says nothing until the project has at
least eight dialect-carrying words, below which a majority is a coincidence.
It says nothing on a close split or a tie, because a project that has not
chosen is not a project in violation. And when it does fire, the message names
the majority spelling rather than a house style.

A team that has chosen can say so:

```toml
[checks."Consistency.SpellingDialect"]
dialect = "british"   # or "american"
```

With a dialect pinned the minimum-occurrence floor and the majority rule both
fall away: every deviation is reported, however few.

## Word list

Deliberately short. The `-ise`/`-ize` family plus the high-frequency pairs that
turn up in software prose — behaviour, colour, catalogue, centre, defence,
artefact and their inflections. It is a list, not a dictionary, and it is meant
to stay that way: a rule that fires on a word nobody meant to standardise is
worse than one that misses a few.

Two pairs are left out on purpose. `licence`/`license` splits by part of speech
in British English, so both spellings are correct in the same document.
`programme`/`program` is not a dialect pair at all where software is concerned
— "program" is the British spelling too.

## Suppression

The usual `// cofferdam-disable-next-line Consistency.SpellingDialect`, or a
severity override for a directory that quotes external prose verbatim.
