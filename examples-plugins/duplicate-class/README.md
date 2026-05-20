# `DuplicateClassName` — cross-file plugin example (cd-9hp.6)

Demonstrates the plugin corpus + finalize pattern: a plugin that
collects state across every file in `run()` and emits cross-file
findings in `finalize()`.

The rule: a class name declared in more than one file is suspicious.
Pick a canonical declaration; rename or delete the others.

## Build + run

```bash
cd examples-plugins/duplicate-class
npm install        # resolves @cofferdam/check-sdk from the workspace
npm run build      # tsc -p .
cofferdam check fixture
```

Expected output: one `Design.DuplicateClassName` finding pointing at
`fixture/a.ts:5` with `related` referencing `fixture/b.ts:5`. The
second class (`Unique`, only in `a.ts`) does not fire.

## How it works

```ts
// run() — once per file
ctx.corpus.append<ClassDecl>("classes", {
  file: file.path,
  name: cls.name,
  span: cls.span,
});

// finalize() — once per analysis run, after every file's run() completes
const all = ctx.corpus.read<ClassDecl[]>("classes") ?? [];
// group by name, emit one finding per cross-file duplicate
```

`ctx.corpus` is plugin-private — two plugins picking the same slot key
do not see each other's data; the host namespaces by `check.id`. See
[docs/plugin-sdk-guide.md](../../docs/plugin-sdk-guide.md) for the full
contract.
