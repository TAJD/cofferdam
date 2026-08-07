//! Project-graph builder — pass 1 extraction of imports and exports.
//!
//! Walks each parsed file once, populates the well-known `IMPORTS` and
//! `EXPORTS` corpus slots in `cofferdam-core::graph`. Module specifiers
//! are resolved through `oxc_resolver` so graph-aware checks see absolute
//! paths instead of raw `./foo` strings — that's what lets a check ask
//! "is anyone importing the symbol I just exported?" without hand-rolling
//! TypeScript's resolution rules.
//!
//! The builder runs INSIDE the engine's per-file pass, before built-in
//! checks see the file. It does NOT emit findings of its own — it's pure
//! evidence collection. Checks consume the slots in `Check::finalize`.
//!
//! ## Resolution
//!
//! Single shared `Resolver` per analysis run. Its options:
//! * Extensions: `.ts .tsx .d.ts .mts .cts .js .jsx .mjs .cjs .json`.
//! * `extension_alias` (CD-139): a `.js`/`.jsx`/`.mjs`/`.cjs` specifier
//!   also tries the matching TS source extension first (`.js` ->
//!   `.ts`/`.tsx`/`.js`) — required for the ESM-standard
//!   `import "./f.js"` pointing at `f.ts` source, or every such import
//!   silently fails to resolve. `extension_alias` matching bypasses the
//!   general extension list entirely, so a `.js`/`.mjs`/`.cjs` specifier
//!   pointing at a declaration-only sibling (`f.d.ts`, `f.d.mts`,
//!   `f.d.cts`) with no `.ts`/`.js` source is intentionally out of scope —
//!   narrow in practice since `Design.OrphanExport` cares about first-party
//!   source, not ambient type declarations.
//! * tsconfig discovery: `Auto` — walks up from the importing file's
//!   directory looking for `tsconfig.json`, honors `paths`/`baseUrl`.
//!
//! Bare specifiers that resolve into `node_modules` are recorded with
//! `resolved = Some(path_inside_node_modules)`. Graph-aware checks that
//! care about "in-project only" filter on path prefix.
//!
//! ## What's recorded
//!
//! | Source form | `IMPORTS` | `EXPORTS` |
//! |---|---|---|
//! | `import x from './m'` | yes (Default) | — |
//! | `import { x } from './m'` | yes (Named, with type-only flag) | — |
//! | `import * as ns from './m'` | yes (Namespace) | — |
//! | `import './polyfill'` | yes (no `names`) | — |
//! | `import type { X }` | yes (`type_only`) | — |
//! | `export const x = …` | — | yes (Named) |
//! | `export function x() {}` | — | yes (Named) |
//! | `export class X {}` | — | yes (Named) |
//! | `export default …` | — | yes (Default, name="default") |
//! | `export { x }` | — | yes (Named) |
//! | `export { x as y } from './m'` | yes (Named) + yes (ReExport, name="y") | — |
//! | `export * from './m'` | yes (Namespace) + yes (ReExport, name="*") | — |
//! | `module.exports = …`, `require(...)`, dynamic `import('...')` | NOT recorded |

use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportKind, ImportRecord, ImportedName, EXPORTS, IMPORTS,
};
use cofferdam_core::parser::{parse_fatal, parse_into, ParsedView};
use cofferdam_core::span_util::span_from_bytes;
use cofferdam_core::{Allocator, CorpusIndex, SourceFile};
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportAllDeclaration, ExportDefaultDeclaration,
    ExportNamedDeclaration, Expression, IdentifierReference, ImportDeclaration,
    ImportDeclarationSpecifier, ImportExpression, ImportOrExportKind, ModuleExportName, Program,
    Statement,
};
use oxc_ast_visit::Visit;
use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use std::collections::HashMap;

/// Single shared resolver for an analysis run.
pub struct GraphBuilder {
    resolver: Resolver,
}

impl GraphBuilder {
    /// Build a resolver with TS-aware extensions and auto tsconfig
    /// discovery. The latter walks up from each importing file's
    /// directory looking for `tsconfig.json` — so `paths`/`baseUrl`
    /// aliases resolve correctly without us threading the project root.
    ///
    /// `extension_alias` (CD-139) mirrors TypeScript's `moduleResolution:
    /// "bundler"`/`"node16"`/`"nodenext"` behavior: a specifier ending in
    /// `.js` (the required ESM extension at the compiled-output layer)
    /// must resolve against a sibling `.ts`/`.tsx` *source* file when one
    /// exists — `import { f } from "./f.js"` resolving to `f.ts`. Without
    /// this, `oxc_resolver`'s plain extension list only ever matches a
    /// literal `.js` file, so every relative import written in this
    /// (extremely common, ESM-project-standard) style silently fails to
    /// resolve — cascading into mass `Design.OrphanExport` false
    /// positives, since the resolver records no edge at all rather than a
    /// wrong one. The literal extension is kept last in each alias list
    /// so a genuine compiled/plain-JS sibling (no `.ts` source) still
    /// resolves exactly as before.
    pub fn new() -> Self {
        let opts = ResolveOptions {
            extensions: vec![
                ".ts".into(),
                ".tsx".into(),
                ".d.ts".into(),
                ".mts".into(),
                ".cts".into(),
                ".js".into(),
                ".jsx".into(),
                ".mjs".into(),
                ".cjs".into(),
                ".json".into(),
            ],
            extension_alias: vec![
                (
                    ".js".into(),
                    vec![".ts".into(), ".tsx".into(), ".js".into()],
                ),
                (".jsx".into(), vec![".tsx".into(), ".jsx".into()]),
                (".mjs".into(), vec![".mts".into(), ".mjs".into()]),
                (".cjs".into(), vec![".cts".into(), ".cjs".into()]),
            ],
            tsconfig: Some(TsconfigDiscovery::Auto),
            ..ResolveOptions::default()
        };
        Self {
            resolver: Resolver::new(opts),
        }
    }

    /// Walk `parsed` and append every static import and export into the
    /// shared `IMPORTS` / `EXPORTS` slots. The lock is held only for the
    /// per-file append, so concurrent per-file work (cd-6ad) won't
    /// serialise on it.
    pub fn collect(&self, file: &SourceFile, parsed: &ParsedView<'_>, corpus: &CorpusIndex) {
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        collect_program(
            file,
            parsed.program,
            &self.resolver,
            &mut imports,
            &mut exports,
        );
        corpus.with_slot(&IMPORTS, |slot| {
            slot.replace_file(file.path.clone(), imports)
        });
        corpus.with_slot(&EXPORTS, |slot| {
            slot.replace_file(file.path.clone(), exports)
        });
    }

    /// Astro frontmatter import extraction (cd-45). `.astro` SFCs mix an
    /// HTML-like template with an ESM frontmatter fence — the template
    /// isn't valid TS/JS, so the file as a whole is never routed through
    /// oxc. The frontmatter fence *is* plain ESM, so we slice just that
    /// region and reuse the same `collect_program` walk as a normal TS
    /// file, appending only the resulting imports into the shared
    /// `IMPORTS` slot.
    ///
    /// Exports are deliberately discarded: Astro pages routinely declare
    /// frontmatter-level metadata (`export const prerender = true`,
    /// `export interface Props`) that has no meaning as a project-graph
    /// export and would otherwise surface as spurious
    /// `Design.OrphanExport` candidates for nearly every page.
    ///
    /// Import spans are relative to the extracted frontmatter slice, not
    /// the full `.astro` file — acceptable because nothing renders
    /// `ImportRecord::span` for these records today (no check declares
    /// `Language::Astro`, so they're only ever read back out of the
    /// canonical graph as edges).
    pub fn collect_astro(&self, file: &SourceFile, corpus: &CorpusIndex) {
        let Some(frontmatter) = extract_astro_frontmatter(&file.text) else {
            return;
        };
        let fm_file = SourceFile::new(file.path.clone(), frontmatter.to_string());
        let allocator = Allocator::default();
        let parsed_return = parse_into(&allocator, &fm_file);
        if parse_fatal(&parsed_return) {
            // Malformed frontmatter — skip silently rather than emitting
            // a Warning.ParseError against a language no check owns.
            return;
        }
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        collect_program(
            &fm_file,
            &parsed_return.program,
            &self.resolver,
            &mut imports,
            &mut exports,
        );
        // `collect_program`'s local-use counting only sees identifier
        // references inside the frontmatter fence itself — it can't see
        // the template body, which is where an imported component is
        // actually referenced (`<MyIssues client:load />`). Left at 0,
        // that undercounts and trips `Refactor.DeadExport`'s "imported
        // but never used" heuristic for every island. Force every name
        // to count as used; we can't verify template usage here, but
        // treating frontmatter imports as call sites (not orphans) is
        // the correct default — that's Design.OrphanExport's job.
        for imp in &mut imports {
            for n in &mut imp.names {
                n.local_use_count = n.local_use_count.max(1);
            }
        }
        corpus.with_slot(&IMPORTS, |slot| {
            slot.replace_file(file.path.clone(), imports)
        });
    }
}

/// Slice the `---\n ... \n---` frontmatter fence off the start of an
/// Astro file's source text. Per the Astro spec the fence must open the
/// file — no leading blank lines or whitespace — so this only matches at
/// byte 0. Returns `None` when the file has no frontmatter (a valid,
/// if unusual, Astro file — e.g. a template-only partial).
fn extract_astro_frontmatter(text: &str) -> Option<&str> {
    let after_open = text.strip_prefix("---")?;
    let after_open = after_open
        .strip_prefix("\r\n")
        .or_else(|| after_open.strip_prefix('\n'))?;
    let close = after_open.find("\n---")?;
    Some(&after_open[..close])
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// `resolve_file` is the variant that honors `TsconfigDiscovery::Auto`,
/// walking up from the importing file to find a `tsconfig.json`. The
/// alternate `resolve(directory, ...)` API skips that — silently — even
/// when Auto is configured. (oxc_resolver 11.x, see comments on the API.)
fn resolve(resolver: &Resolver, file_path: &Path, specifier: &str) -> Option<PathBuf> {
    resolver
        .resolve_file(file_path, specifier)
        .ok()
        .map(|res| res.full_path())
}

fn export_module_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(i) => i.name.as_str().to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.as_str().to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().to_string(),
    }
}

fn collect_program(
    file: &SourceFile,
    program: &Program<'_>,
    resolver: &Resolver,
    imports: &mut Vec<ImportRecord>,
    exports: &mut Vec<ExportRecord>,
) {
    for stmt in &program.body {
        let Statement::ImportDeclaration(decl) = stmt else {
            // Pattern-match on module declarations directly (oxc 0.128
            // flattens them as Statement variants for ES modules).
            match stmt {
                Statement::ExportNamedDeclaration(decl) => {
                    handle_export_named(file, decl, resolver, imports, exports);
                }
                Statement::ExportDefaultDeclaration(decl) => {
                    handle_export_default(file, decl, exports);
                }
                Statement::ExportAllDeclaration(decl) => {
                    handle_export_all(file, decl, resolver, imports, exports);
                }
                _ => {}
            }
            continue;
        };
        handle_import(file, decl, resolver, imports);
    }

    // Walk for dynamic `import('./m')` calls anywhere in the tree. They
    // resolve to a module namespace so we record them as Namespace
    // imports — `import('./m').then(({ x }) => …)` does in fact touch
    // `x`. Without this, code-splitting patterns (React.lazy, Next.js
    // dynamic) trigger a flood of orphan-export false positives.
    let mut walker = DynamicImportWalker {
        file,
        resolver,
        imports,
        _phantom: std::marker::PhantomData,
    };
    walker.visit_program(program);

    // Count IdentifierReference hits per imported local-name, skipping
    // the import statements themselves so the import declaration's own
    // bindings don't artificially inflate use_count. Used by
    // Refactor.DeadExport to spot imports that are imported then never
    // referenced.
    count_local_uses(file, program, imports);
}

fn count_local_uses(file: &SourceFile, program: &Program<'_>, imports: &mut [ImportRecord]) {
    let mut tally: HashMap<String, u32> = HashMap::new();
    for imp in imports.iter() {
        // Only count for imports that originated in *this* file. Other
        // files' records may already be in the slice when called from
        // a shared corpus path.
        if imp.from_file != file.path {
            continue;
        }
        for n in &imp.names {
            if n.local_name == "*" {
                continue;
            }
            tally.entry(n.local_name.clone()).or_insert(0);
        }
    }
    if tally.is_empty() {
        return;
    }
    let mut counter = UseCounter {
        tally: &mut tally,
        in_import: false,
    };
    counter.visit_program(program);
    for imp in imports.iter_mut() {
        if imp.from_file != file.path {
            continue;
        }
        for n in &mut imp.names {
            if let Some(&c) = tally.get(&n.local_name) {
                n.local_use_count = c;
            }
        }
    }
}

struct UseCounter<'a> {
    tally: &'a mut HashMap<String, u32>,
    in_import: bool,
}

impl<'a, 'ast> Visit<'ast> for UseCounter<'a> {
    fn visit_import_declaration(&mut self, node: &ImportDeclaration<'ast>) {
        // Don't descend into import bindings — those would be miscounted
        // as references to their own declarations.
        let was = self.in_import;
        self.in_import = true;
        oxc_ast_visit::walk::walk_import_declaration(self, node);
        self.in_import = was;
    }

    fn visit_identifier_reference(&mut self, node: &IdentifierReference<'ast>) {
        if !self.in_import {
            if let Some(c) = self.tally.get_mut(node.name.as_str()) {
                *c += 1;
            }
        }
    }
}

struct DynamicImportWalker<'a, 'ast> {
    file: &'a SourceFile,
    resolver: &'a Resolver,
    imports: &'a mut Vec<ImportRecord>,
    _phantom: std::marker::PhantomData<&'ast ()>,
}

impl<'a, 'ast> Visit<'ast> for DynamicImportWalker<'a, 'ast> {
    fn visit_import_expression(&mut self, node: &ImportExpression<'ast>) {
        if let Expression::StringLiteral(s) = &node.source {
            let specifier = s.value.as_str().to_string();
            let resolved = resolve(self.resolver, &self.file.path, &specifier);
            self.imports.push(ImportRecord {
                from_file: self.file.path.clone(),
                source_specifier: specifier,
                resolved,
                names: vec![ImportedName {
                    source_name: "*".to_string(),
                    local_name: "*".to_string(),
                    kind: ImportKind::Namespace,
                    type_only: false,
                    local_use_count: 0,
                }],
                type_only: false,
                span: span_from_bytes(&self.file.text, node.span.start, node.span.end),
            });
        }
        oxc_ast_visit::walk::walk_import_expression(self, node);
    }
}

fn handle_import(
    file: &SourceFile,
    decl: &ImportDeclaration<'_>,
    resolver: &Resolver,
    imports: &mut Vec<ImportRecord>,
) {
    let resolved = resolve(resolver, &file.path, decl.source.value.as_str());
    let mut names: Vec<ImportedName> = Vec::new();
    if let Some(specifiers) = &decl.specifiers {
        for spec in specifiers {
            match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    names.push(ImportedName {
                        source_name: export_module_name(&s.imported),
                        local_name: s.local.name.as_str().to_string(),
                        kind: ImportKind::Named,
                        type_only: matches!(s.import_kind, ImportOrExportKind::Type),
                        local_use_count: 0,
                    });
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    names.push(ImportedName {
                        source_name: "default".to_string(),
                        local_name: s.local.name.as_str().to_string(),
                        kind: ImportKind::Default,
                        type_only: false,
                        local_use_count: 0,
                    });
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    names.push(ImportedName {
                        source_name: "*".to_string(),
                        local_name: s.local.name.as_str().to_string(),
                        kind: ImportKind::Namespace,
                        type_only: false,
                        local_use_count: 0,
                    });
                }
            }
        }
    }
    imports.push(ImportRecord {
        from_file: file.path.clone(),
        source_specifier: decl.source.value.as_str().to_string(),
        resolved,
        names,
        type_only: matches!(decl.import_kind, ImportOrExportKind::Type),
        span: span_from_bytes(&file.text, decl.span.start, decl.span.end),
    });
}

fn handle_export_named(
    file: &SourceFile,
    decl: &ExportNamedDeclaration<'_>,
    resolver: &Resolver,
    imports: &mut Vec<ImportRecord>,
    exports: &mut Vec<ExportRecord>,
) {
    let type_only = matches!(decl.export_kind, ImportOrExportKind::Type);

    // `export { x } from './y'` — also records an implicit Named import
    // so reachability analysis can walk the chain.
    if let Some(source) = &decl.source {
        let specifier = source.value.as_str().to_string();
        let resolved = resolve(resolver, &file.path, &specifier);
        let mut names: Vec<ImportedName> = Vec::new();
        for spec in &decl.specifiers {
            let local = export_module_name(&spec.local);
            let exported = export_module_name(&spec.exported);
            names.push(ImportedName {
                source_name: local.clone(),
                local_name: exported.clone(),
                kind: ImportKind::Named,
                type_only: type_only || matches!(spec.export_kind, ImportOrExportKind::Type),
                local_use_count: 0,
            });
            exports.push(ExportRecord {
                file: file.path.clone(),
                name: exported,
                kind: ExportKind::ReExport,
                type_only: type_only || matches!(spec.export_kind, ImportOrExportKind::Type),
                span: span_from_bytes(&file.text, spec.span.start, spec.span.end),
                source_specifier: Some(specifier.clone()),
                resolved_source: resolved.clone(),
            });
        }
        imports.push(ImportRecord {
            from_file: file.path.clone(),
            source_specifier: specifier,
            resolved,
            names,
            type_only,
            span: span_from_bytes(&file.text, decl.span.start, decl.span.end),
        });
        return;
    }

    // `export { x }` — pure local re-exports of in-file bindings, no source.
    for spec in &decl.specifiers {
        let exported = export_module_name(&spec.exported);
        exports.push(ExportRecord {
            file: file.path.clone(),
            name: exported,
            kind: ExportKind::Named,
            type_only: type_only || matches!(spec.export_kind, ImportOrExportKind::Type),
            span: span_from_bytes(&file.text, spec.span.start, spec.span.end),
            source_specifier: None,
            resolved_source: None,
        });
    }

    // `export <decl>` — pull the bound name(s).
    if let Some(declaration) = &decl.declaration {
        push_declaration_exports(file, declaration, type_only, exports);
    }
}

fn push_declaration_exports(
    file: &SourceFile,
    decl: &Declaration<'_>,
    type_only: bool,
    exports: &mut Vec<ExportRecord>,
) {
    match decl {
        Declaration::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                exports.push(ExportRecord {
                    file: file.path.clone(),
                    name: id.name.as_str().to_string(),
                    kind: ExportKind::Named,
                    type_only,
                    span: span_from_bytes(&file.text, id.span.start, id.span.end),
                    source_specifier: None,
                    resolved_source: None,
                });
            }
        }
        Declaration::ClassDeclaration(c) => {
            if let Some(id) = &c.id {
                exports.push(ExportRecord {
                    file: file.path.clone(),
                    name: id.name.as_str().to_string(),
                    kind: ExportKind::Named,
                    type_only,
                    span: span_from_bytes(&file.text, id.span.start, id.span.end),
                    source_specifier: None,
                    resolved_source: None,
                });
            }
        }
        Declaration::VariableDeclaration(v) => {
            for d in &v.declarations {
                push_binding_exports(file, &d.id, type_only, exports);
            }
        }
        Declaration::TSTypeAliasDeclaration(t) => {
            exports.push(ExportRecord {
                file: file.path.clone(),
                name: t.id.name.as_str().to_string(),
                kind: ExportKind::Named,
                type_only: true,
                span: span_from_bytes(&file.text, t.id.span.start, t.id.span.end),
                source_specifier: None,
                resolved_source: None,
            });
        }
        Declaration::TSInterfaceDeclaration(t) => {
            exports.push(ExportRecord {
                file: file.path.clone(),
                name: t.id.name.as_str().to_string(),
                kind: ExportKind::Named,
                type_only: true,
                span: span_from_bytes(&file.text, t.id.span.start, t.id.span.end),
                source_specifier: None,
                resolved_source: None,
            });
        }
        Declaration::TSEnumDeclaration(t) => {
            exports.push(ExportRecord {
                file: file.path.clone(),
                name: t.id.name.as_str().to_string(),
                kind: ExportKind::Named,
                type_only,
                span: span_from_bytes(&file.text, t.id.span.start, t.id.span.end),
                source_specifier: None,
                resolved_source: None,
            });
        }
        Declaration::TSModuleDeclaration(t) => {
            // Only the named `namespace X {}` form has an exportable name —
            // module-string forms (`declare module 'foo'`) don't.
            if let oxc_ast::ast::TSModuleDeclarationName::Identifier(ident) = &t.id {
                exports.push(ExportRecord {
                    file: file.path.clone(),
                    name: ident.name.as_str().to_string(),
                    kind: ExportKind::Named,
                    type_only,
                    span: span_from_bytes(&file.text, ident.span.start, ident.span.end),
                    source_specifier: None,
                    resolved_source: None,
                });
            }
        }
        _ => {}
    }
}

fn push_binding_exports(
    file: &SourceFile,
    pattern: &BindingPattern<'_>,
    type_only: bool,
    exports: &mut Vec<ExportRecord>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(id) => {
            exports.push(ExportRecord {
                file: file.path.clone(),
                name: id.name.as_str().to_string(),
                kind: ExportKind::Named,
                type_only,
                span: span_from_bytes(&file.text, id.span.start, id.span.end),
                source_specifier: None,
                resolved_source: None,
            });
        }
        BindingPattern::ObjectPattern(obj) => {
            for prop in &obj.properties {
                push_binding_exports(file, &prop.value, type_only, exports);
            }
            if let Some(rest) = &obj.rest {
                push_binding_exports(file, &rest.argument, type_only, exports);
            }
        }
        BindingPattern::ArrayPattern(arr) => {
            for elem in arr.elements.iter().flatten() {
                push_binding_exports(file, elem, type_only, exports);
            }
            if let Some(rest) = &arr.rest {
                push_binding_exports(file, &rest.argument, type_only, exports);
            }
        }
        BindingPattern::AssignmentPattern(assign) => {
            push_binding_exports(file, &assign.left, type_only, exports);
        }
    }
}

fn handle_export_default(
    file: &SourceFile,
    decl: &ExportDefaultDeclaration<'_>,
    exports: &mut Vec<ExportRecord>,
) {
    exports.push(ExportRecord {
        file: file.path.clone(),
        name: "default".to_string(),
        kind: ExportKind::Default,
        type_only: false,
        span: span_from_bytes(&file.text, decl.span.start, decl.span.end),
        source_specifier: None,
        resolved_source: None,
    });
}

fn handle_export_all(
    file: &SourceFile,
    decl: &ExportAllDeclaration<'_>,
    resolver: &Resolver,
    imports: &mut Vec<ImportRecord>,
    exports: &mut Vec<ExportRecord>,
) {
    let specifier = decl.source.value.as_str().to_string();
    let resolved = resolve(resolver, &file.path, &specifier);
    let type_only = matches!(decl.export_kind, ImportOrExportKind::Type);

    // Star re-export records as a Namespace import (so reachability sees
    // the whole-module touch) plus a wildcard export.
    let exported_name = decl
        .exported
        .as_ref()
        .map(export_module_name)
        .unwrap_or_else(|| "*".to_string());
    exports.push(ExportRecord {
        file: file.path.clone(),
        name: exported_name,
        kind: ExportKind::ReExport,
        type_only,
        span: span_from_bytes(&file.text, decl.span.start, decl.span.end),
        source_specifier: Some(specifier.clone()),
        resolved_source: resolved.clone(),
    });
    imports.push(ImportRecord {
        from_file: file.path.clone(),
        source_specifier: specifier,
        resolved,
        names: vec![ImportedName {
            source_name: "*".to_string(),
            local_name: "*".to_string(),
            kind: ImportKind::Namespace,
            type_only: false,
            local_use_count: 0,
        }],
        type_only,
        span: span_from_bytes(&file.text, decl.span.start, decl.span.end),
    });
}

#[cfg(test)]
mod extension_alias_tests {
    use super::*;
    use cofferdam_core::CorpusIndex;

    /// Writes `importer` (given a `SourceFile` name) importing `specifier`,
    /// plus every file in `siblings` (name -> content) into a fresh
    /// tempdir, runs `GraphBuilder::collect` on the importer, and returns
    /// the single recorded import's `resolved` path (relative to the
    /// tempdir, for assertion stability).
    fn resolve_specifier(
        importer_content: &str,
        siblings: &[(&str, &str)],
    ) -> (tempfile::TempDir, Option<PathBuf>) {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, content) in siblings {
            std::fs::write(dir.path().join(name), content).expect("write sibling");
        }
        let importer_path = dir.path().join("importer.ts");
        std::fs::write(&importer_path, importer_content).expect("write importer");

        let file = SourceFile::new(
            importer_path.clone(),
            std::fs::read_to_string(&importer_path).unwrap(),
        );
        let allocator = Allocator::default();
        let parsed_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parsed_return.program,
            diagnostics: &parsed_return.errors,
        };
        let corpus = CorpusIndex::new();
        let builder = GraphBuilder::new();
        builder.collect(&file, &parsed, &corpus);

        let imports: Vec<_> = corpus.with_slot(&IMPORTS, |slot| slot.records().cloned().collect());
        assert_eq!(imports.len(), 1, "expected exactly one import: {imports:?}");
        (dir, imports[0].resolved.clone())
    }

    #[test]
    fn js_specifier_resolves_to_sibling_ts_source() {
        let (dir, resolved) = resolve_specifier(
            "import { f } from \"./f.js\";\nf();\n",
            &[("f.ts", "export function f(): void {}\n")],
        );
        assert_eq!(resolved, Some(dir.path().join("f.ts")));
    }

    #[test]
    fn jsx_specifier_resolves_to_sibling_tsx_source() {
        let (dir, resolved) = resolve_specifier(
            "import { C } from \"./C.jsx\";\nC;\n",
            &[("C.tsx", "export const C = () => null;\n")],
        );
        assert_eq!(resolved, Some(dir.path().join("C.tsx")));
    }

    #[test]
    fn mjs_specifier_resolves_to_sibling_mts_source() {
        let (dir, resolved) = resolve_specifier(
            "import { g } from \"./g.mjs\";\ng();\n",
            &[("g.mts", "export function g(): void {}\n")],
        );
        assert_eq!(resolved, Some(dir.path().join("g.mts")));
    }

    #[test]
    fn cjs_specifier_resolves_to_sibling_cts_source() {
        let (dir, resolved) = resolve_specifier(
            "import { h } from \"./h.cjs\";\nh();\n",
            &[("h.cts", "export function h(): void {}\n")],
        );
        assert_eq!(resolved, Some(dir.path().join("h.cts")));
    }

    /// A `.ts` sibling wins over a same-named literal `.js` file when
    /// both exist (matches `tsc`'s own resolution order: source before
    /// compiled output).
    #[test]
    fn js_specifier_prefers_ts_sibling_over_literal_js_sibling() {
        let (dir, resolved) = resolve_specifier(
            "import { f } from \"./f.js\";\nf();\n",
            &[
                ("f.ts", "export function f(): void {}\n"),
                ("f.js", "export function f() {}\n"),
            ],
        );
        assert_eq!(resolved, Some(dir.path().join("f.ts")));
    }

    /// Plain-JS projects (no `.ts` source anywhere) must keep resolving a
    /// literal `.js` specifier to its literal `.js` file — the alias list
    /// keeps `.js` itself as a fallback precisely so this doesn't
    /// regress.
    #[test]
    fn js_specifier_falls_back_to_literal_js_file_when_no_ts_sibling() {
        let (dir, resolved) = resolve_specifier(
            "import { f } from \"./f.js\";\nf();\n",
            &[("f.js", "export function f() {}\n")],
        );
        assert_eq!(resolved, Some(dir.path().join("f.js")));
    }
}

#[cfg(test)]
mod astro_tests {
    use super::*;
    use cofferdam_core::CorpusIndex;

    #[test]
    fn extracts_frontmatter_between_fences() {
        let text = "---\nimport X from './x';\n---\n<X />\n";
        assert_eq!(
            extract_astro_frontmatter(text),
            Some("import X from './x';")
        );
    }

    #[test]
    fn none_when_no_opening_fence() {
        assert_eq!(extract_astro_frontmatter("<h1>Hello</h1>\n"), None);
    }

    #[test]
    fn none_when_fence_unclosed() {
        assert_eq!(
            extract_astro_frontmatter("---\nimport X from './x';\n<X />\n"),
            None
        );
    }

    #[test]
    fn collect_astro_records_frontmatter_import_but_no_exports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let page = dir.path().join("page.astro");
        let island = dir.path().join("Island.tsx");
        std::fs::write(
            &island,
            "export default function Island() { return null; }\n",
        )
        .expect("write island");
        std::fs::write(
            &page,
            "---\nimport Island from './Island';\nexport const prerender = true;\n---\n<Island />\n",
        )
        .expect("write page");

        let file = SourceFile::new(page.clone(), std::fs::read_to_string(&page).unwrap());
        let corpus = CorpusIndex::new();
        let builder = GraphBuilder::new();
        builder.collect_astro(&file, &corpus);

        let imports: Vec<_> = corpus.with_slot(&IMPORTS, |slot| slot.records().cloned().collect());
        assert_eq!(imports.len(), 1, "expected one import record: {imports:?}");
        assert_eq!(imports[0].resolved.as_deref(), Some(island.as_path()));
        assert!(
            imports[0].names.iter().all(|n| n.local_use_count > 0),
            "frontmatter imports must be marked used — template usage is invisible \
             to the frontmatter parse, so Refactor.DeadExport must not flag them: {imports:?}"
        );

        let exports: Vec<_> = corpus.with_slot(&EXPORTS, |slot| slot.records().cloned().collect());
        assert!(
            exports.is_empty(),
            "frontmatter exports must not be recorded: {exports:?}"
        );
    }
}
