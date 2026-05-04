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
use cofferdam_core::parser::ParsedView;
use cofferdam_core::span_util::span_from_bytes;
use cofferdam_core::{CorpusIndex, SourceFile};
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
        if !imports.is_empty() {
            corpus.with_slot(&IMPORTS, |slot| slot.append(&mut imports));
        }
        if !exports.is_empty() {
            corpus.with_slot(&EXPORTS, |slot| slot.append(&mut exports));
        }
    }
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
