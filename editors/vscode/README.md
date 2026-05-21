# cofferdam VS Code extension (smoke-test stub)

Not published to the VS Code marketplace. This is the cd-9hp.4 cp5
shim that exercises the `cofferdam lsp` server end-to-end in a real
editor.

## Run it

```bash
cd editors/vscode
npm install
npm run compile
cd ../..
cargo build --release -p cofferdam-cli   # produces target/release/cofferdam(.exe)
```

Then launch VS Code's Extension Development Host pointing at this
directory:

```bash
code --extensionDevelopmentPath=editors/vscode <path-to-a-ts-project>
```

In the dev host, set `cofferdam.executable` in workspace settings to
the absolute path of the binary you built (`target/release/cofferdam`
or `target/release/cofferdam.exe` on Windows). Open a TypeScript file;
diagnostics from cofferdam land in the Problems panel.

## What this is not

- Not published. Don't `vsce publish` from this directory.
- Not configured for marketplace icons / branding.
- Not packaged with the binary — `cofferdam.executable` must point at
  a separately-built binary (or `cofferdam` on PATH).
