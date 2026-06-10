# Installing cofferdam

```sh
npm install --save-dev @cofferdam/cofferdam
pnpm add -D @cofferdam/cofferdam
yarn add --dev @cofferdam/cofferdam
```

The `postinstall` script downloads the matching prebuilt binary for your platform: Linux x64/arm64 (glibc and musl), macOS x64/arm64, Windows x64. Node 16+ is required for the wrapper; the binary itself has no runtime dependencies.

The legacy unscoped `cofferdam` npm package was deprecated at v0.2.2 — use the scoped name.

## Binary overrides

Two environment variables let you bypass the postinstall download:

- `COFFERDAM_BINARY_PATH=/abs/path` — skip the download and use this binary instead. Useful for testing a locally built binary against an npm-installed wrapper.
- `COFFERDAM_SKIP_DOWNLOAD=1` — skip postinstall entirely, for CI or Docker layers where you supply the binary another way.

## Air-gapped installs

Download the archive for your platform from the [GitHub Release](https://github.com/TAJD/cofferdam/releases), extract it, then point `COFFERDAM_BINARY_PATH` at the extracted binary and run:

```sh
npm rebuild @cofferdam/cofferdam
```

## Building from source

Requires Rust 1.93+ (the declared MSRV, enforced in CI):

```sh
cargo install --path crates/cofferdam-cli
```

from a repository checkout.
