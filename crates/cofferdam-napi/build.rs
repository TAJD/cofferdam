// build.rs — wires napi-build setup so the resulting cdylib is loadable
// as a Node.js native addon.
//
// On Windows, napi-build links against libnode.dll which lives in the
// Node.js source distribution, NOT a default install. Setting it up
// requires either:
//
//   1. The `@napi-rs/cli` workflow (recommended; sets the env vars).
//   2. Hand-installing the Node dev files via node-gyp or downloading
//      the Node source archive and pointing `NAPI_NODE_DEV_DIR` at it.
//
// To opt out of the cdylib link step locally — the Rust code still
// compiles as an rlib — set `COFFERDAM_SKIP_NAPI_LINK=1`. The macros
// continue to register exports; only the final .node link is skipped.

extern crate napi_build;

fn main() {
    if std::env::var_os("COFFERDAM_SKIP_NAPI_LINK").is_some() {
        // Don't try to find libnode.dll — useful for workspace builds on
        // a host that doesn't have the Node dev environment set up.
        println!("cargo:rerun-if-env-changed=COFFERDAM_SKIP_NAPI_LINK");
        return;
    }
    napi_build::setup();
}
