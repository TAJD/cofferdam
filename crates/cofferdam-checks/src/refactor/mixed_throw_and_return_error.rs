use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    Expression, Function, IfStatement, ObjectPropertyKind, PropertyKey, ReturnStatement,
    ThrowStatement,
};
use oxc_ast_visit::Visit;

/// Names an error-shaped object literal must carry a field named
/// exactly one of these to count as "returning an error" for this
/// heuristic — kept narrow per the ticket's guidance to start strict.
const ERROR_SHAPE_FIELDS: &[&str] = &["error", "ok", "success"];

/// `Refactor.MixedThrowAndReturnError` — flag a function that both
/// `throw`s and `return`s an error-shaped object literal (a field named
/// `error`, `ok`, or `success`) from genuinely distinct branches — two
/// different idioms for signaling the same class of failure, which
/// hurts composability of error paths for callers.
///
/// AST-only, name/shape heuristic:
/// - Only inspects a function's own statements — nested functions
///   (declarations, expressions, arrow functions) are separate scopes
///   and are not descended into; each is analyzed independently when
///   the outer walk reaches it.
/// - "Distinct branches" is approximated by assigning each `if`
///   consequent/alternate its own id (regardless of whether it's
///   braced) and each nested `BlockStatement` its own id. A throw and a
///   qualifying return that land in the exact same block/branch are
///   NOT flagged — that's usually dead code (the second is
///   unreachable), not two competing error idioms.
/// - A field named `error`/`ok`/`success` only counts as
///   "error-shaped" for values that actually signal failure:
///   `{ error: null }` / `{ error: undefined }` and `{ ok: true }` /
///   `{ success: true }` are the success arm of a Result-shaped return,
///   not a competing error idiom, and are excluded.
pub struct MixedThrowAndReturnError;

const META: CheckMeta = CheckMeta {
    id: "Refactor.MixedThrowAndReturnError",
    category: Category::Refactor,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A function that both throws and returns an error-shaped object for what looks \
        like the same class of failure mixes two error-handling idioms, hurting composability \
        of error paths for callers.",
    body: include_str!("../../docs/Refactor.MixedThrowAndReturnError.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for MixedThrowAndReturnError {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = TopLevelVisitor {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

/// A field named `error`/`ok`/`success` signals *failure* only for
/// certain values — `{ error: null }` and `{ ok: true }` are the
/// SUCCESS arm of a Result-shaped return, not a competing error idiom,
/// so they're explicitly excluded even though the field name matches.
fn signals_failure(field_name: &str, value: &Expression<'_>) -> bool {
    match field_name {
        "error" => {
            !matches!(value, Expression::NullLiteral(_))
                && !matches!(value, Expression::Identifier(id) if id.name.as_str() == "undefined")
        }
        "ok" | "success" => !matches!(value, Expression::BooleanLiteral(lit) if lit.value),
        _ => true,
    }
}

fn is_error_shaped(expr: &Expression<'_>) -> bool {
    let Expression::ObjectExpression(obj) = expr else {
        return false;
    };
    obj.properties.iter().any(|prop| {
        let ObjectPropertyKind::ObjectProperty(prop) = prop else {
            return false;
        };
        let name: Option<&str> = match &prop.key {
            PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
            PropertyKey::StringLiteral(lit) => Some(lit.value.as_str()),
            _ => None,
        };
        name.is_some_and(|n| ERROR_SHAPE_FIELDS.contains(&n) && signals_failure(n, &prop.value))
    })
}

/// Walks the whole program, running a fresh `FnScanner` on every
/// function-like node it reaches, then continues descending so nested
/// functions get their own independent scan.
struct TopLevelVisitor<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for TopLevelVisitor<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let mut scanner = FnScanner {
            block_stack: Vec::new(),
            next_block_id: 1,
            throw_blocks: Vec::new(),
            error_return_blocks: Vec::new(),
        };
        if let Some(body) = &node.body {
            for stmt in &body.statements {
                scanner.visit_statement(stmt);
            }
        }
        let flagged = scanner
            .throw_blocks
            .iter()
            .any(|t| scanner.error_return_blocks.iter().any(|r| t != r));
        if flagged {
            let name = node
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string())
                .unwrap_or_else(|| "(anonymous)".to_string());
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "function `{name}` both throws and returns an error-shaped object — pick \
                     one error-handling idiom"
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(META.base_priority),
                severity: Severity::Low,
                related: Vec::new(),
            });
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }
}

/// Scans a single function's own body (skipping nested function-like
/// scopes) for throw sites and error-shaped return sites, tagging each
/// with the id of its innermost enclosing block.
struct FnScanner {
    block_stack: Vec<u32>,
    next_block_id: u32,
    throw_blocks: Vec<u32>,
    error_return_blocks: Vec<u32>,
}

impl FnScanner {
    fn current_block(&self) -> u32 {
        *self.block_stack.last().unwrap_or(&0)
    }
}

impl FnScanner {
    fn with_new_block<F: FnOnce(&mut Self)>(&mut self, f: F) {
        let id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(id);
        f(self);
        self.block_stack.pop();
    }
}

impl<'a> Visit<'a> for FnScanner {
    fn visit_block_statement(&mut self, node: &oxc_ast::ast::BlockStatement<'a>) {
        self.with_new_block(|this| oxc_ast_visit::walk::walk_block_statement(this, node));
    }

    /// A brace-less `if` consequent/alternate (`if (c) throw x;`) is not
    /// its own `BlockStatement` — assign each branch its own block id
    /// here regardless of braces, so branch divergence is detected the
    /// same way whether or not the arms use `{}`.
    fn visit_if_statement(&mut self, node: &IfStatement<'a>) {
        self.visit_expression(&node.test);
        self.with_new_block(|this| this.visit_statement(&node.consequent));
        if let Some(alternate) = &node.alternate {
            self.with_new_block(|this| this.visit_statement(alternate));
        }
    }

    fn visit_throw_statement(&mut self, node: &ThrowStatement<'a>) {
        self.throw_blocks.push(self.current_block());
        oxc_ast_visit::walk::walk_throw_statement(self, node);
    }

    fn visit_return_statement(&mut self, node: &ReturnStatement<'a>) {
        if let Some(arg) = &node.argument {
            if is_error_shaped(arg) {
                self.error_return_blocks.push(self.current_block());
            }
        }
        oxc_ast_visit::walk::walk_return_statement(self, node);
    }

    fn visit_function(&mut self, _node: &Function<'a>, _flags: oxc_syntax::scope::ScopeFlags) {
        // Nested function — separate scope, not this function's own
        // throw/return sites. The outer TopLevelVisitor analyzes it.
    }

    fn visit_arrow_function_expression(
        &mut self,
        _node: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        // Same reasoning as visit_function above.
    }
}
