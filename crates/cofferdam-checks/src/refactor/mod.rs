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
mod mutated_parameter;
mod prefer_nullish_coalescing;
mod prefer_optional_chain;
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
pub use mutated_parameter::MutatedParameter;
pub use prefer_nullish_coalescing::PreferNullishCoalescing;
pub use prefer_optional_chain::PreferOptionalChain;
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
}
