use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, OptionDefault, OptionKind,
    OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, BindingPattern, Declaration, ExportNamedDeclaration,
    Function, IdentifierReference, Program, SimpleAssignmentTarget, Statement, UpdateExpression,
    VariableDeclarationKind,
};
use oxc_ast_visit::Visit;
use std::collections::HashSet;

/// `Refactor.PurityHeuristic` — flag an exported function that reads a
/// module-level `let` binding not covered by its own parameter list, AND
/// that binding is reassigned somewhere in the file. A fuzzy heuristic
/// for a hidden dependency on outside-the-signature state.
///
/// Ship opt-in: disabled by default (the `enabled` option must be set
/// `true` in `cofferdam.toml`) pending real-repo false-positive data,
/// per the ticket's explicit high-false-positive-risk warning.
///
/// Deliberately excludes read-only module state (a binding declared
/// `let` but never reassigned anywhere): memoization caches, singletons,
/// config objects, and loggers are legitimate read-only dependencies,
/// not hidden mutation coupling. Only a binding that is BOTH declared
/// with `let` at module scope AND written to somewhere in the file
/// counts as the "outside state" this check cares about.
///
/// Name-based, not binding-based (mirrors `Refactor.PreferConstOverLet`
/// and `Refactor.MutatedParameter`'s documented trade-offs). Two
/// consequences worth knowing before enabling this check:
///
/// - A function parameter or local variable that shadows a flagged
///   module-level name is indistinguishable from it by this check's own
///   parameter-name exclusion, but a *nested local* declared inside the
///   function body (not a parameter) sharing that name is NOT
///   distinguished — reading such a shadowing local would still flag.
/// - "Written to somewhere in the file" is also name-based, not
///   scope-based: an unrelated local variable that happens to share a
///   module-level `let`'s name (in a different function, different
///   scope) still counts as a "write" to that name, which can pull a
///   genuinely read-only module binding (a config object, a logger)
///   into `mutable_module_state` and cause the false positive this
///   check is explicitly supposed to avoid.
///
/// Both are known, deliberately-accepted false-positive edge cases,
/// consistent with the check's opt-in-only status.
///
/// Scope of what's inspected: only inline `export function foo() {}`
/// declarations are analyzed. `export default function foo() {}` and a
/// separate `function foo() {} ... export { foo };` are NOT inspected —
/// a false-negative gap, not a correctness bug.
pub struct PurityHeuristic;

pub const PURITY_HEURISTIC_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "enabled",
    kind: OptionKind::Bool,
    default: OptionDefault::Bool(false),
    doc: "opt in to this fuzzy, high-false-positive-risk heuristic (disabled by default)",
}];

const META: CheckMeta = CheckMeta {
    id: "Refactor.PurityHeuristic",
    category: Category::Refactor,
    base_priority: -10,
    default_severity: Severity::Low,
    explanation: "An exported function reads a module-level mutable binding not covered by \
        its own parameter list — a hidden dependency on outside-the-signature state that works \
        against unit-testability.",
    body: include_str!("../../docs/Refactor.PurityHeuristic.md"),
    requires_types: false,
    consistency: false,
    options: PURITY_HEURISTIC_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl Check for PurityHeuristic {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        if !ctx.options.get_bool("enabled").unwrap_or(false) {
            return Vec::new();
        }
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };

        let module_let_names = top_level_let_names(parsed.program);
        if module_let_names.is_empty() {
            return Vec::new();
        }

        let mut mutation_collector = MutationCollector {
            reassigned: HashSet::new(),
        };
        mutation_collector.visit_program(parsed.program);
        let mutable_module_state: HashSet<String> = module_let_names
            .intersection(&mutation_collector.reassigned)
            .cloned()
            .collect();
        if mutable_module_state.is_empty() {
            return Vec::new();
        }

        let mut visitor = Collector {
            file,
            mutable_module_state,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

/// Module-level (top of `Program.body`, not nested in any function or
/// block) `let`-declared simple-identifier names — both bare (`let x`)
/// and inline-exported (`export let x`, which oxc represents as an
/// `ExportNamedDeclaration` wrapping the `VariableDeclaration`).
fn top_level_let_names(program: &Program<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::VariableDeclaration(decl) => Some(decl.as_ref()),
            Statement::ExportNamedDeclaration(export) => match &export.declaration {
                Some(Declaration::VariableDeclaration(decl)) => Some(decl.as_ref()),
                _ => None,
            },
            _ => None,
        };
        let Some(decl) = decl else { continue };
        if decl.kind == VariableDeclarationKind::Let {
            for d in &decl.declarations {
                if let BindingPattern::BindingIdentifier(id) = &d.id {
                    names.insert(id.name.as_str().to_string());
                }
            }
        }
    }
    names
}

fn simple_param_names(func: &Function<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    for p in &func.params.items {
        if let BindingPattern::BindingIdentifier(id) = &p.pattern {
            names.insert(id.name.as_str().to_string());
        }
    }
    names
}

/// Whole-file pass: every name ever the target of an assignment or
/// increment/decrement, regardless of scope (same name-based trade-off
/// as `Refactor.PreferConstOverLet`).
struct MutationCollector {
    reassigned: HashSet<String>,
}

impl<'a> Visit<'a> for MutationCollector {
    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &node.left {
            self.reassigned.insert(id.name.as_str().to_string());
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument {
            self.reassigned.insert(id.name.as_str().to_string());
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    mutable_module_state: HashSet<String>,
    issues: Vec<Issue>,
}

impl<'a> Collector<'a> {
    fn check_exported_function(&mut self, func: &Function<'a>) {
        let Some(id) = &func.id else { return };
        let Some(body) = &func.body else { return };
        let params = simple_param_names(func);

        let mut reads = ReadCollector {
            reads: HashSet::new(),
        };
        for stmt in &body.statements {
            reads.visit_statement(stmt);
        }

        let mut hidden_deps: Vec<&str> = self
            .mutable_module_state
            .iter()
            .filter(|name| reads.reads.contains(*name) && !params.contains(*name))
            .map(String::as_str)
            .collect();
        hidden_deps.sort_unstable();
        if hidden_deps.is_empty() {
            return;
        }

        let span = span_from_bytes(&self.file.text, func.span.start, func.span.end);
        self.issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!(
                "exported function `{}` reads module-level mutable state ({}) not covered by \
                 its parameter list — a hidden dependency working against unit-testability",
                id.name.as_str(),
                hidden_deps.join(", ")
            ),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(META.base_priority),
            severity: Severity::Low,
            related: Vec::new(),
        });
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_export_named_declaration(&mut self, node: &ExportNamedDeclaration<'a>) {
        if let Some(Declaration::FunctionDeclaration(func)) = &node.declaration {
            self.check_exported_function(func);
        }
        oxc_ast_visit::walk::walk_export_named_declaration(self, node);
    }
}

/// Every identifier read (as an `IdentifierReference`, not a binding
/// site) within a function body.
///
/// A plain `=` assignment target (`x = 1`) is a pure write, not a read,
/// even though oxc's walk still visits it as an `IdentifierReference` —
/// so it's explicitly skipped here. A compound assignment (`x += 1`) or
/// update expression (`x++`) genuinely reads the prior value before
/// writing, so those targets are left to the default walk.
struct ReadCollector {
    reads: HashSet<String>,
}

impl<'a> Visit<'a> for ReadCollector {
    fn visit_identifier_reference(&mut self, node: &IdentifierReference<'a>) {
        self.reads.insert(node.name.as_str().to_string());
    }

    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        if node.operator == oxc_ast::ast::AssignmentOperator::Assign {
            self.visit_expression(&node.right);
            return;
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }
}
