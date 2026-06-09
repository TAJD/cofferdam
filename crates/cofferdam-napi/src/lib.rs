//! `cofferdam-napi` — napi-rs FFI surface (phase 4).
//!
//! Will expose the engine and plugin host to Node, so user-authored TS
//! checks can be loaded into worker_threads without leaving the napi-rs
//! prebuilt-binary distribution path. Empty until phase 4 lands.

#[cfg(test)]
mod tests {
    // Compile-guard for cd-9r3 (phase-4 MCP/napi FFI work).
    //
    // No assertions here — the test's job is to break the build loudly
    // if Engine::new's signature changes, rather than letting this crate
    // rot silently while the engine API evolves around it.
    #[test]
    fn engine_api_is_reachable() {
        // Cheapest observable entry point: construct an engine with no checks.
        let _engine = cofferdam_engine::Engine::new(vec![]);
    }
}
