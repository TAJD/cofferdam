//! Refactor checks — mechanical cleanups, often autofixable.
//!
//! Cyclomatic and cognitive complexity both walk function-like nodes
//! and tally a per-function score. They differ only in scoring rules:
//! McCabe cyclomatic counts independent paths flatly; Sonar cognitive
//! adds a nesting penalty so deeply-nested branching costs more than
//! a long flat switch.
//!
//! Both checks ignore code outside any function (top-level statements
//! at module scope) — the metrics are designed for callable units.

mod cognitive_complexity;
mod cyclomatic_complexity;
mod dead_export;
mod duplicate_block;
mod long_and_complex;
mod mixed_throw_and_return_error;
mod mutated_parameter;
mod prefer_array_method_over_loop;
mod prefer_const_over_let;
mod prefer_nullish_coalescing;
mod prefer_optional_chain;
mod purity_heuristic;
mod side_effect_in_map_callback;
mod unused_variable;

pub use cognitive_complexity::{
    max_in_file as max_cognitive_complexity_in_file, CognitiveComplexity,
};
pub use cyclomatic_complexity::{
    max_in_file as max_cyclomatic_complexity_in_file, CyclomaticComplexity,
};
pub use dead_export::DeadExport;
pub use duplicate_block::DuplicateBlock;
pub use long_and_complex::LongAndComplex;
pub use mixed_throw_and_return_error::MixedThrowAndReturnError;
pub use mutated_parameter::MutatedParameter;
pub use prefer_array_method_over_loop::PreferArrayMethodOverLoop;
pub use prefer_const_over_let::PreferConstOverLet;
pub use prefer_nullish_coalescing::PreferNullishCoalescing;
pub use prefer_optional_chain::PreferOptionalChain;
pub use purity_heuristic::{PurityHeuristic, PURITY_HEURISTIC_OPTIONS};
pub use side_effect_in_map_callback::SideEffectInMapCallback;
pub use unused_variable::UnusedVariable;

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::validate_options;
    use cofferdam_core::{
        parser::parse_into, parser::ParsedView, Allocator, Check, CheckContext, CheckOptions,
        CorpusIndex, FinalizeContext, Issue as CoreIssue, RawOptionValue, SourceFile,
    };
    use duplicate_block::{
        hash_token_window, tokenise, AstHashWalker, DuplicateBlock, DUP_BLOCK_OPTIONS,
    };
    use oxc_ast_visit::Visit;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use unused_variable::UnusedVariable;

    #[test]
    fn tokenise_canonicalises_identifiers() {
        let toks = tokenise("const foo = bar + foo;");
        // const $_0 = $_1 + $_0 ;
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(canons, vec!["const", "$_0", "=", "$_1", "+", "$_0", ";"]);
    }

    #[test]
    fn tokenise_keeps_keywords_and_literals() {
        let toks = tokenise(r#"if (x === 5) return "hi";"#);
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["if", "(", "$_0", "=", "=", "=", "5", ")", "return", "\"hi\"", ";"]
        );
    }

    #[test]
    fn tokenise_strips_comments_and_whitespace() {
        let toks = tokenise("// leading\nconst x = 1; /* block */ const y = 2;");
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["const", "$_0", "=", "1", ";", "const", "$_1", "=", "2", ";"]
        );
    }

    #[test]
    fn rename_equivalent_windows_hash_equal() {
        let a = tokenise("const total = price + tax; return total;");
        let b = tokenise("const sum = amount + fee; return sum;");
        // Both canonicalise to: const $_0 = $_1 + $_2 ; return $_0 ;
        assert_eq!(hash_token_window(&a), hash_token_window(&b));
    }

    #[test]
    fn distinct_structure_hashes_differ() {
        let a = tokenise("const x = 1; const y = 2;");
        let b = tokenise("const x = 1; let y = 2;"); // const vs let
        assert_ne!(hash_token_window(&a), hash_token_window(&b));
    }

    #[test]
    fn tokenise_handles_unicode_identifiers() {
        // Greek letters + Han characters as identifier names. The
        // pre-cd-s2k ASCII-only scanner would treat each non-ASCII byte
        // as a single-byte operator token; with unicode-ident, they
        // form proper identifier tokens that get canonicalised.
        let toks = tokenise("const λ = 1; const 計算 = λ + 1;");
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["const", "$_0", "=", "1", ";", "const", "$_1", "=", "$_0", "+", "1", ";"]
        );
        // Byte spans land on char boundaries, never mid-codepoint.
        for t in &toks {
            assert!(t.end as usize <= "const λ = 1; const 計算 = λ + 1;".len());
        }
    }

    #[test]
    fn rename_equivalent_unicode_and_ascii_match() {
        let ascii = tokenise("function add(x, y) { return x + y; }");
        let greek = tokenise("function add(α, β) { return α + β; }");
        assert_eq!(hash_token_window(&ascii), hash_token_window(&greek));
    }

    // ─── AST hash walker (cd-mti) tests ────────────────────────────────────
    //
    // Run a quick parse on a synthetic source, then compare hashes to verify
    // structural canonicalisation does what we expect.

    fn ast_hash_first_n_stmts(text: &str, n: usize) -> u64 {
        let file = SourceFile::new(PathBuf::from("test.ts"), text.to_string());
        let alloc = Allocator::default();
        let parsed = parse_into(&alloc, &file);
        let mut walker = AstHashWalker::new();
        for stmt in parsed.program.body.iter().take(n) {
            walker.visit_statement(stmt);
        }
        walker.finish()
    }

    #[test]
    fn ast_hash_rename_equivalent_matches() {
        let a = "const total = price + tax; return total;";
        let b = "const sum = amount + fee; return sum;";
        assert_eq!(ast_hash_first_n_stmts(a, 2), ast_hash_first_n_stmts(b, 2));
    }

    #[test]
    fn ast_hash_brace_style_collapses() {
        // Block-with-braces vs single-statement consequent. The text
        // canonicaliser produced different hashes for these (because
        // `{` and `}` were tokens); the AST hasher should treat them
        // the same: an `if` with an ExpressionStatement consequent.
        //
        // (oxc actually wraps a single-statement consequent in a
        // BlockStatement only if braces are present in source, so the
        // two forms DO differ structurally — making this a useful
        // sanity check that the new hasher correctly distinguishes
        // them rather than hashes them equal.)
        let with_braces = "if (a) { b(); }";
        let without_braces = "if (a) b();";
        assert_ne!(
            ast_hash_first_n_stmts(with_braces, 1),
            ast_hash_first_n_stmts(without_braces, 1),
            "Block vs single-statement consequents are structurally different \
             AST shapes — they should hash distinctly"
        );
    }

    #[test]
    fn ast_hash_distinguishes_call_from_member() {
        // Same identifier, different shape: function call vs member access
        // SHOULD hash differently.
        let call = "foo(x);";
        let member = "foo.x;";
        assert_ne!(
            ast_hash_first_n_stmts(call, 1),
            ast_hash_first_n_stmts(member, 1)
        );
    }

    #[test]
    fn ast_hash_distinguishes_let_from_const() {
        let l = "let x = 1;";
        let c = "const x = 1;";
        assert_ne!(ast_hash_first_n_stmts(l, 1), ast_hash_first_n_stmts(c, 1));
    }

    #[test]
    fn ast_hash_distinguishes_literal_values() {
        let five = "if (x === 5) return;";
        let six = "if (x === 6) return;";
        assert_ne!(
            ast_hash_first_n_stmts(five, 1),
            ast_hash_first_n_stmts(six, 1)
        );
    }

    // ─── DuplicateBlock option wiring tests (cd-rt6) ───────────────────────
    //
    // Run the full run+finalize cycle on two files that share duplicate
    // statement blocks, with overridden options, and assert the option
    // values are picked up correctly.

    /// Duplicate snippet large enough to fire at default thresholds
    /// (6 statements, >80 chars).
    fn make_duplicate_source() -> String {
        // Six structurally identical statements, each substantial enough to
        // clear the min_chars floor when combined.
        [
            "const alpha = getValue(source);",
            "const beta = transform(alpha);",
            "const gamma = validate(beta);",
            "const delta = normalize(gamma);",
            "const epsilon = format(delta);",
            "const result = finalize(epsilon);",
        ]
        .join("\n")
    }

    /// Run DuplicateBlock on two copies of the same source and return
    /// the issues emitted by finalize.
    fn run_duplicate_block_with_options(
        check: &DuplicateBlock,
        source: &str,
        options: &CheckOptions,
    ) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::default();

        let alloc_a = Allocator::default();
        let file_a = SourceFile::new(PathBuf::from("a.ts"), source.to_string());
        let ret_a = parse_into(&alloc_a, &file_a);
        let view_a = ParsedView {
            program: &ret_a.program,
            diagnostics: &ret_a.errors,
        };
        let mut ctx_a = CheckContext::new(&file_a)
            .with_parsed(&view_a)
            .with_options(options)
            .with_corpus(&corpus);
        check.run(&file_a, &mut ctx_a);

        let alloc_b = Allocator::default();
        let file_b = SourceFile::new(PathBuf::from("b.ts"), source.to_string());
        let ret_b = parse_into(&alloc_b, &file_b);
        let view_b = ParsedView {
            program: &ret_b.program,
            diagnostics: &ret_b.errors,
        };
        let mut ctx_b = CheckContext::new(&file_b)
            .with_parsed(&view_b)
            .with_options(options)
            .with_corpus(&corpus);
        check.run(&file_b, &mut ctx_b);

        let mut finalize_ctx = FinalizeContext::new(&corpus);
        check.finalize(&mut finalize_ctx)
    }

    #[test]
    fn duplicate_block_default_options_fires_on_duplicate() {
        let check = DuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let issues = run_duplicate_block_with_options(&check, &make_duplicate_source(), &opts);
        assert!(
            !issues.is_empty(),
            "expected at least one DuplicateBlock issue with default options"
        );
    }

    #[test]
    fn duplicate_block_high_min_statements_suppresses_finding() {
        // min_statements=100 means our 6-statement duplicate is too short.
        let check = DuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("min_statements".to_string(), RawOptionValue::Int(100));
        let opts = validate_options("Refactor.DuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        let issues = run_duplicate_block_with_options(&check, &make_duplicate_source(), &opts);
        assert!(
            issues.is_empty(),
            "expected no issues when min_statements=100 but got {}",
            issues.len()
        );
    }

    #[test]
    fn duplicate_block_min_statements_one_fires_on_tiny_duplicate() {
        // min_statements=1 + min_chars=1 fires even on a single substantial statement.
        let check = DuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("min_statements".to_string(), RawOptionValue::Int(1));
        raw.insert("min_chars".to_string(), RawOptionValue::Int(1));
        let opts = validate_options("Refactor.DuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        let source = "const alpha = getValue(source);";
        let issues = run_duplicate_block_with_options(&check, source, &opts);
        assert!(
            !issues.is_empty(),
            "expected at least one issue with min_statements=1, min_chars=1"
        );
    }

    #[test]
    fn duplicate_block_include_ast_false_produces_no_ast_findings() {
        // With include_ast=false and include_tokens=false (default), nothing fires.
        let check = DuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("include_ast".to_string(), RawOptionValue::Bool(false));
        let opts = validate_options("Refactor.DuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        let issues = run_duplicate_block_with_options(&check, &make_duplicate_source(), &opts);
        assert!(
            issues.is_empty(),
            "expected no issues when include_ast=false and include_tokens=false (default)"
        );
    }

    #[test]
    fn duplicate_block_include_tokens_option_is_read() {
        // Confirm the include_tokens option is picked up by running with a
        // large min_statements (disabling AST mode) but include_tokens=true.
        // Token mode operates on different thresholds; the test only verifies
        // that the option value is threaded through without panicking.
        let check = DuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("include_tokens".to_string(), RawOptionValue::Bool(true));
        raw.insert("include_ast".to_string(), RawOptionValue::Bool(false));
        let opts = validate_options("Refactor.DuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        // Should not panic; token-mode may or may not produce issues for this source.
        let _ = run_duplicate_block_with_options(&check, &make_duplicate_source(), &opts);
    }

    // ─── UnusedVariable: TS parameter properties (cd-sh72 / gh #44) ─────────
    //
    // A parameter property (`constructor(private ctx: T)`) is both a
    // constructor param AND a class field. oxc resolves `this.ctx` as a
    // member access, not a reference to the parameter symbol, so the bare
    // get_resolved_references signal misses the read and the param is
    // flagged as unused. These tests pin the fix: a `this.<name>` read in
    // the enclosing class counts as use of the parameter property.

    fn run_unused_variable(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        UnusedVariable.run(&file, &mut ctx)
    }

    #[test]
    fn param_property_read_via_this_is_not_flagged() {
        let src = "\
class Foo {
  constructor(private ctx: { value: number }) {}
  getValue(): number {
    return this.ctx.value;
  }
}";
        let issues = run_unused_variable(src);
        assert!(
            issues.iter().all(|i| !i.message.contains("`ctx`")),
            "param property `ctx` is read via this.ctx and must not flag; got: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn param_property_never_read_is_flagged() {
        let src = "\
class Bar {
  constructor(private unused: number) {}
}";
        let issues = run_unused_variable(src);
        assert!(
            issues.iter().any(|i| i.message.contains("`unused`")),
            "param property `unused` is never read and must still flag; got: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn plain_unused_constructor_param_is_flagged() {
        // No access modifier => not a parameter property => normal
        // positional-parameter rules apply (flag when unused).
        let src = "\
class Baz {
  constructor(ctx: number) {}
}";
        let issues = run_unused_variable(src);
        assert!(
            issues.iter().any(|i| i.message.contains("`ctx`")),
            "plain unused constructor param `ctx` must still flag; got: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn param_property_this_access_is_scoped_per_class() {
        // Same name `ctx` in two classes: A reads `this.ctx` (used), B never
        // does (unused). Only B's `ctx` may flag — proves the this-access
        // set is scoped to the enclosing class, not the whole file. A
        // file-level set would exempt both (zero flags); the bug flags both.
        let src = "\
class A {
  constructor(private ctx: number) {}
  read(): number { return this.ctx; }
}
class B {
  constructor(private ctx: number) {}
}";
        let issues = run_unused_variable(src);
        let ctx_issue_count = issues
            .iter()
            .filter(|i| i.message.contains("`ctx`"))
            .count();
        assert_eq!(
            ctx_issue_count,
            1,
            "exactly one `ctx` (class B's) should flag; got: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    // ─── Refactor.PreferConstOverLet (CD-119) ──────────────────────────

    use prefer_const_over_let::PreferConstOverLet;

    fn run_prefer_const_over_let(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        PreferConstOverLet.run(&file, &mut ctx)
    }

    #[test]
    fn never_reassigned_let_is_flagged() {
        let issues = run_prefer_const_over_let("let total = 1 + 2;\nreturn total;");
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("total"));
    }

    #[test]
    fn directly_reassigned_let_is_not_flagged() {
        let issues = run_prefer_const_over_let("let price = 1;\nprice = price - 1;\nreturn price;");
        assert!(issues.is_empty(), "expected no findings; got {issues:?}");
    }

    #[test]
    fn incremented_let_is_not_flagged() {
        let issues = run_prefer_const_over_let(
            "let count = 0;\nwhile (count < 5) { count++; }\nreturn count;",
        );
        assert!(issues.is_empty(), "expected no findings; got {issues:?}");
    }

    #[test]
    fn let_reassigned_in_nested_closure_is_not_flagged() {
        let src = "\
function makeCounter() {
  let count = 0;
  return function increment() {
    count += 1;
    return count;
  };
}";
        let issues = run_prefer_const_over_let(src);
        assert!(
            issues.is_empty(),
            "closure reassignment of a captured outer let must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn const_binding_is_never_flagged() {
        let issues = run_prefer_const_over_let("const total = 1 + 2;\nreturn total;");
        assert!(
            issues.is_empty(),
            "const bindings are out of scope; got {issues:?}"
        );
    }

    #[test]
    fn non_overlapping_same_named_lets_both_flagged() {
        // Two unrelated `let total`s in different functions, neither ever
        // reassigned — both must flag independently, not just the first.
        let src = "\
export function a() { let total = 1; return total; }
export function b() { let total = 2; return total; }";
        let issues = run_prefer_const_over_let(src);
        assert_eq!(
            issues.len(),
            2,
            "expected both unrelated `total` lets to flag; got {issues:?}"
        );
    }

    #[test]
    fn destructured_let_binding_is_skipped() {
        let issues = run_prefer_const_over_let("let { a, b } = obj;\nreturn a + b;");
        assert!(
            issues.is_empty(),
            "destructured bindings are MVP-skipped; got {issues:?}"
        );
    }

    // ─── Refactor.PreferArrayMethodOverLoop (CD-120) ───────────────────

    use prefer_array_method_over_loop::PreferArrayMethodOverLoop;

    fn run_prefer_array_method_over_loop(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        PreferArrayMethodOverLoop.run(&file, &mut ctx)
    }

    #[test]
    fn inline_push_loop_is_flagged_as_map() {
        let src = "\
const doubled: number[] = [];
for (const n of nums) {
  doubled.push(n * 2);
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains(".map()"));
    }

    #[test]
    fn computed_then_push_loop_is_flagged_as_map() {
        let src = "\
const labels: string[] = [];
for (const n of nums) {
  const label = String(n);
  labels.push(label);
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains(".map()"));
    }

    #[test]
    fn if_gated_push_loop_is_flagged_as_filter() {
        let src = "\
const evens: number[] = [];
for (const n of nums) {
  if (n % 2 === 0) {
    evens.push(n);
  }
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains(".filter()"));
    }

    #[test]
    fn if_gated_computed_push_loop_is_flagged_as_filter_map() {
        // The if's consequent pushes a separately computed transform, not
        // the raw loop variable — the suggestion must be `.filter().map()`,
        // not a plain `.filter()`, since the loop does both.
        let src = "\
const labels: string[] = [];
for (const n of nums) {
  if (n % 2 === 0) {
    const label = `even:${n}`;
    labels.push(label);
  }
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(
            issues[0].message.contains(".filter().map()"),
            "expected a filter+map suggestion, not a plain filter; got: {}",
            issues[0].message
        );
    }

    #[test]
    fn multi_arg_push_is_not_flagged() {
        // arr.push(a, b) pushes two items per call — not map/filter-shaped.
        let src = "\
const flat: number[] = [];
for (const n of nums) {
  flat.push(n, n + 1);
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert!(
            issues.is_empty(),
            "multi-arg push must not flag; got {issues:?}"
        );
    }

    #[test]
    fn loop_with_break_is_not_flagged() {
        let src = "\
const evens: number[] = [];
for (const n of nums) {
  if (evens.length >= limit) {
    break;
  }
  if (n % 2 === 0) {
    evens.push(n);
  }
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert!(
            issues.is_empty(),
            "early break must disqualify the match; got {issues:?}"
        );
    }

    #[test]
    fn loop_with_two_accumulators_is_not_flagged() {
        let src = "\
const evens: number[] = [];
const odds: number[] = [];
for (const n of nums) {
  if (n % 2 === 0) {
    evens.push(n);
  } else {
    odds.push(n);
  }
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert!(
            issues.is_empty(),
            "if/else with two accumulators must not flag; got {issues:?}"
        );
    }

    #[test]
    fn loop_with_extra_side_effect_is_not_flagged() {
        let src = "\
const copy: number[] = [];
for (const n of nums) {
  console.log(n);
  copy.push(n);
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert!(
            issues.is_empty(),
            "a side effect beyond the single push must disqualify the match; got {issues:?}"
        );
    }

    #[test]
    fn loop_without_push_is_not_flagged() {
        let src = "\
let total = 0;
for (let i = 0; i < nums.length; i++) {
  total += nums[i];
}";
        let issues = run_prefer_array_method_over_loop(src);
        assert!(
            issues.is_empty(),
            "a loop that never pushes must not flag; got {issues:?}"
        );
    }

    // ─── Refactor.PurityHeuristic (CD-123) ──────────────────────────────

    use purity_heuristic::PurityHeuristic;

    fn run_purity_heuristic(src: &str, enabled: bool) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("enabled".to_string(), RawOptionValue::Bool(enabled));
        let opts =
            validate_options("Refactor.PurityHeuristic", PURITY_HEURISTIC_OPTIONS, &raw).unwrap();
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&parsed)
            .with_options(&opts);
        PurityHeuristic.run(&file, &mut ctx)
    }

    #[test]
    fn disabled_by_default_emits_nothing() {
        let src = "\
let requestCount = 0;
export function logRequest(name: string) {
  console.log(name, requestCount);
}
export function bump() {
  requestCount += 1;
}";
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        let issues = PurityHeuristic.run(&file, &mut ctx);
        assert!(
            issues.is_empty(),
            "check must be a no-op without explicit opt-in; got {issues:?}"
        );
    }

    #[test]
    fn exported_function_reading_mutated_module_let_is_flagged() {
        let src = "\
let requestCount = 0;
export function logRequest(name: string) {
  console.log(name, requestCount);
}
requestCount = 1;";
        let issues = run_purity_heuristic(src, true);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn compound_assignment_to_module_state_counts_as_a_read() {
        // `bump` writes requestCount via `+=`, which reads the prior
        // value first — a genuine (if self-inflicted) hidden dependency.
        let src = "\
let requestCount = 0;
export function bump() {
  requestCount += 1;
}";
        let issues = run_purity_heuristic(src, true);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn read_only_module_state_is_not_flagged() {
        let src = "\
let config = { apiUrl: \"https://api.example.com\" };
export function fetchData() {
  return fetch(config.apiUrl);
}";
        let issues = run_purity_heuristic(src, true);
        assert!(
            issues.is_empty(),
            "a module-level let that's never reassigned must not flag; got {issues:?}"
        );
    }

    #[test]
    fn parameter_shadowing_module_name_is_not_flagged() {
        let src = "\
let total = 0;
export function resetTotal() {
  total = 0;
}
export function addToTotal(total: number) {
  return total + 1;
}";
        let issues = run_purity_heuristic(src, true);
        assert_eq!(
            issues.len(),
            0,
            "own parameter covers the read even though the module-level name is mutated \
             elsewhere; got {issues:?}"
        );
    }

    #[test]
    fn exported_let_module_state_is_still_detected() {
        // `export let` wraps the VariableDeclaration in an
        // ExportNamedDeclaration — must still be picked up as
        // module-level mutable state.
        let src = "\
export let requestCount = 0;
export function logRequest(name: string) {
  console.log(name, requestCount);
}
requestCount = 1;";
        let issues = run_purity_heuristic(src, true);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    // ─── Refactor.MixedThrowAndReturnError (CD-124) ─────────────────────

    use mixed_throw_and_return_error::MixedThrowAndReturnError;

    fn run_mixed_throw_and_return_error(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        MixedThrowAndReturnError.run(&file, &mut ctx)
    }

    #[test]
    fn throw_and_error_return_in_distinct_branches_is_flagged() {
        let src = "\
function parseConfig(input: string) {
  if (!input) {
    throw new Error(\"input required\");
  }
  if (!input.length) {
    return { error: \"invalid config\" };
  }
  return input;
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn error_return_only_no_throw_is_not_flagged() {
        let src = "function loadResult() { return { error: null, value: 42 }; }";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "no throw anywhere must not flag; got {issues:?}"
        );
    }

    #[test]
    fn throw_only_no_error_return_is_not_flagged() {
        let src = "function f(x: number) { if (x < 0) { throw new Error(\"bad\"); } return x; }";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "no error-shaped return anywhere must not flag; got {issues:?}"
        );
    }

    #[test]
    fn throw_and_error_return_in_same_block_is_not_flagged() {
        let src = "\
function overlapping(x: number) {
  if (x < 0) {
    throw new Error(\"negative\");
    return { error: \"unreachable\" };
  }
  return x * 2;
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "throw and the only error-shaped return sharing a block is dead code, not a \
             competing idiom; got {issues:?}"
        );
    }

    #[test]
    fn unrelated_object_shape_is_not_flagged() {
        let src = "\
function f(x: number) {
  if (x < 0) {
    throw new Error(\"bad\");
  }
  return { total: x };
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "an object return without an error/ok/success field must not flag; got {issues:?}"
        );
    }

    #[test]
    fn nested_function_throw_does_not_leak_into_outer_function() {
        let src = "\
function outer(x: number) {
  function inner() {
    throw new Error(\"inner failure\");
  }
  inner();
  return { error: \"outer failure\" };
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "a throw inside a nested function is that function's own scope, not outer's; \
             got {issues:?}"
        );
        // The nested function itself has neither an error-shaped return nor
        // a second throw, so it isn't independently flagged either.
    }

    #[test]
    fn result_shape_success_return_is_not_flagged() {
        // `{ error: null }` is the SUCCESS arm of a Result-shaped return,
        // not a competing error idiom, even though a throw exists nearby
        // for an unrelated invariant.
        let src = "\
function divide(a: number, b: number) {
  if (b === 0) {
    throw new Error(\"division by zero\");
  }
  return { error: null, value: a / b };
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "`error: null` signals success, not failure; got {issues:?}"
        );
    }

    #[test]
    fn ok_true_success_return_is_not_flagged() {
        let src = "\
function f(x: number) {
  if (x < 0) {
    throw new Error(\"bad\");
  }
  return { ok: true, value: x };
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert!(
            issues.is_empty(),
            "`ok: true` signals success, not failure; got {issues:?}"
        );
    }

    #[test]
    fn ok_false_failure_return_is_flagged() {
        let src = "\
function f(x: number) {
  if (x < 0) {
    throw new Error(\"bad\");
  }
  if (x > 100) {
    return { ok: false };
  }
  return { ok: true, value: x };
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert_eq!(
            issues.len(),
            1,
            "`ok: false` genuinely signals failure, alongside a throw; got {issues:?}"
        );
    }

    #[test]
    fn brace_less_guard_clauses_in_distinct_branches_are_flagged() {
        let src = "\
function parseId(raw: string) {
  if (!raw) throw new Error(\"id required\");
  if (raw.length > 64) return { error: \"id too long\" };
  return raw;
}";
        let issues = run_mixed_throw_and_return_error(src);
        assert_eq!(
            issues.len(),
            1,
            "brace-less guard clauses must still be treated as distinct branches; got {issues:?}"
        );
    }

    // ─── Refactor.SideEffectInMapCallback (CD-125) ──────────────────────

    use side_effect_in_map_callback::SideEffectInMapCallback;

    fn run_side_effect_in_map_callback(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        SideEffectInMapCallback.run(&file, &mut ctx)
    }

    #[test]
    fn map_callback_mutating_outer_scope_is_flagged() {
        let src = "\
const seen = [];
const doubled = items.map((item) => {
  seen.push(item);
  return item * 2;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn filter_callback_calling_console_is_flagged() {
        let src = "\
const positives = items.filter((item) => {
  console.log(\"checking\", item);
  return item > 0;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn map_callback_with_only_local_state_is_not_flagged() {
        let src = "\
const scaled = items.map((item) => {
  const local = item * 2;
  return local;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert!(
            issues.is_empty(),
            "mutating only its own locals must not flag; got {issues:?}"
        );
    }

    #[test]
    fn nested_function_mutation_does_not_leak_into_outer_callback() {
        let src = "\
const wrapped = items.map((item) => {
  function makeLabel() {
    let label = \"\";
    label += String(item);
    return label;
  }
  return makeLabel();
});";
        let issues = run_side_effect_in_map_callback(src);
        assert!(
            issues.is_empty(),
            "a nested function mutating its own local must not flag the outer callback; \
             got {issues:?}"
        );
    }

    #[test]
    fn foreach_callback_is_never_inspected() {
        let src = "\
const seen = [];
items.forEach((item) => {
  seen.push(item);
});";
        let issues = run_side_effect_in_map_callback(src);
        assert!(
            issues.is_empty(),
            ".forEach is exempt — a side effect there is the point; got {issues:?}"
        );
    }

    #[test]
    fn discarded_return_with_no_other_side_effect_is_not_flagged() {
        let src = "items.map((item) => process(item));";
        let issues = run_side_effect_in_map_callback(src);
        assert!(
            issues.is_empty(),
            "a discarded-return misuse with no real side effect is a distinct concern (\"use \
             forEach instead\"), not this check's job; got {issues:?}"
        );
    }

    #[test]
    fn member_assignment_to_outer_object_is_flagged() {
        let src = "\
const state = { count: 0 };
const withCount = items.map((item) => {
  state.count += 1;
  return item;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert_eq!(
            issues.len(),
            1,
            "writing into an outer-scope object property must flag; got {issues:?}"
        );
    }

    #[test]
    fn computed_index_assignment_to_outer_array_is_flagged() {
        let src = "\
const acc = [];
const withIndexWrite = items.map((item, i) => {
  acc[i] = item;
  return item;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert_eq!(
            issues.len(),
            1,
            "writing into an outer-scope array by index must flag; got {issues:?}"
        );
    }

    #[test]
    fn member_assignment_to_own_local_is_not_flagged() {
        let src = "\
const result = items.map((item) => {
  const local = { count: 0 };
  local.count += 1;
  return local.count + item;
});";
        let issues = run_side_effect_in_map_callback(src);
        assert!(
            issues.is_empty(),
            "writing into the callback's own local object must not flag; got {issues:?}"
        );
    }

    #[test]
    fn expression_body_arrow_side_effect_is_flagged() {
        let src = "\
const seen = [];
const doubled = items.map((item) => seen.push(item));";
        let issues = run_side_effect_in_map_callback(src);
        assert_eq!(
            issues.len(),
            1,
            "an expression-body arrow callback must be inspected the same as a block-body one; \
             got {issues:?}"
        );
    }
}
