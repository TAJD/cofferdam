# cofferdam

TypeScript code-quality analyzer — Rust core, JS plugin layer.

```bash
pnpm add -D cofferdam
pnpm exec cofferdam check src/
```

The npm package is a thin wrapper. On install, a postinstall script downloads the matching pre-built binary from the [GitHub Release](https://github.com/TAJD/cofferdam/releases) for your platform (Linux x64/arm64 gnu+musl, macOS x64/arm64, Windows x64). The binary lands in `node_modules/cofferdam/bin/` and is invoked through the `cofferdam` shim.

Full project documentation: https://github.com/TAJD/cofferdam

## Usage

```bash
pnpm exec cofferdam check src/                 # human report
pnpm exec cofferdam check src/ --robot         # JSON for AI agents / CI
pnpm exec cofferdam check                      # walk current dir
pnpm exec cofferdam hello                      # banner
```

Exit codes: `0` no findings, `1` findings present, `2` invocation/IO error.

## Configuration

`cofferdam.toml` at the project root. Schema reference is in the main repo's docs (incoming). Defaults work for most TypeScript projects without configuration.

## CI

```yaml
- run: pnpm exec cofferdam check src/ --robot > findings.json
- uses: actions/upload-artifact@v4
  with:
    name: cofferdam-findings
    path: findings.json
```

## Sandboxed installs / `--ignore-scripts`

If your installer disables postinstall scripts, the binary won't be downloaded. Two recovery paths:

1. **Manual binary**: download the release archive yourself, extract, and set `COFFERDAM_BINARY_PATH` to the binary, then `npm rebuild cofferdam`.
2. **Skip download for build images**: set `COFFERDAM_SKIP_DOWNLOAD=1` if you've baked the binary into the image at `node_modules/cofferdam/bin/cofferdam`.

## Versioning

The npm package version tracks the cofferdam release version. `cofferdam@0.1.0` downloads the binary from the `v0.1.0` GitHub Release. Lockfile-pinned installs are deterministic.

## License

MIT.
