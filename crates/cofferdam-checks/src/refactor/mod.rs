//! Refactor checks — mechanical cleanups, often autofixable.

mod dead_export;
mod duplicate_block;
mod long_and_complex;
mod mixed_throw_and_return_error;
mod purity_heuristic;
mod side_effect_in_map_callback;

pub use dead_export::DeadExport;
pub use duplicate_block::NearDuplicateBlock;
pub use long_and_complex::LongAndComplex;
pub use mixed_throw_and_return_error::MixedThrowAndReturnError;
pub use purity_heuristic::{PurityHeuristic, PURITY_HEURISTIC_OPTIONS};
pub use side_effect_in_map_callback::SideEffectInMapCallback;

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::validate_options;
    use cofferdam_core::{
        parser::parse_into, parser::ParsedView, Allocator, Check, CheckContext, CheckOptions,
        CorpusIndex, FinalizeContext, Issue as CoreIssue, RawOptionValue, SourceFile,
    };
    use duplicate_block::{
        finalize_all_groups_for_test, hash_token_window, tokenise, AstHashWalker,
        NearDuplicateBlock, DUP_BLOCK_OPTIONS,
    };
    use oxc_ast_visit::Visit;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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

    fn ast_hash_first_n_stmts(text: &str, n: usize) -> (u64, u64) {
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

    /// Run NearDuplicateBlock on two copies of the same source and
    /// return every claimed group's issue (exact + near — see
    /// `finalize_all_groups_for_test`), used to exercise the shared
    /// windowing/collection logic regardless of which half a fixture
    /// lands in.
    fn run_duplicate_block_with_options(
        check: &NearDuplicateBlock,
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
        finalize_all_groups_for_test(&mut finalize_ctx)
    }

    /// Run NearDuplicateBlock on a single file and return the issues
    /// emitted by finalize — used to check that a genuinely
    /// non-duplicated block doesn't report itself as a duplicate of its
    /// own sliding windows.
    fn run_duplicate_block_single_file(
        check: &NearDuplicateBlock,
        source: &str,
        options: &CheckOptions,
    ) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::default();
        let alloc = Allocator::default();
        let file = SourceFile::new(PathBuf::from("a.ts"), source.to_string());
        let ret = parse_into(&alloc, &file);
        let view = ParsedView {
            program: &ret.program,
            diagnostics: &ret.errors,
        };
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&view)
            .with_options(options)
            .with_corpus(&corpus);
        check.run(&file, &mut ctx);

        let mut finalize_ctx = FinalizeContext::new(&corpus);
        finalize_all_groups_for_test(&mut finalize_ctx)
    }

    #[test]
    fn duplicate_block_field_copy_chain_does_not_self_overlap() {
        // CD-147: property names canonicalise the same way identifiers do
        // (`visit_identifier_name`), so a uniform field-copy chain like
        // this makes every 6-statement sliding window hash identically —
        // each window has the same shape with local identifier indices
        // restarting at 0. Before the fix, those overlapping windows
        // landed in one hash group and finalize reported the single real
        // block as a "duplicate" of itself.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source = [
            "target.alpha = source.alpha;",
            "target.beta = source.beta;",
            "target.gamma = source.gamma;",
            "target.delta = source.delta;",
            "target.epsilon = source.epsilon;",
            "target.zeta = source.zeta;",
            "target.eta = source.eta;",
            "target.theta = source.theta;",
        ]
        .join("\n");
        let issues = run_duplicate_block_single_file(&check, &source, &opts);
        assert!(
            issues.is_empty(),
            "a single non-duplicated field-copy chain must not self-report as a duplicate; got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_field_copy_chain_still_detects_real_cross_file_duplicate() {
        // Same self-overlapping shape as above, but genuinely duplicated
        // across two files — the self-overlap fix must not suppress a
        // real cross-file duplicate, and must report it exactly once
        // (not once per overlapping window pair).
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source = [
            "target.alpha = source.alpha;",
            "target.beta = source.beta;",
            "target.gamma = source.gamma;",
            "target.delta = source.delta;",
            "target.epsilon = source.epsilon;",
            "target.zeta = source.zeta;",
            "target.eta = source.eta;",
            "target.theta = source.theta;",
        ]
        .join("\n");
        let issues = run_duplicate_block_with_options(&check, &source, &opts);
        assert_eq!(
            issues.len(),
            1,
            "expected exactly one duplicate finding for the real cross-file copy, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_default_options_fires_on_duplicate() {
        let check = NearDuplicateBlock::default();
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
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("min_statements".to_string(), RawOptionValue::Int(100));
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
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
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("min_statements".to_string(), RawOptionValue::Int(1));
        raw.insert("min_chars".to_string(), RawOptionValue::Int(1));
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
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
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("include_ast".to_string(), RawOptionValue::Bool(false));
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
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
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("include_tokens".to_string(), RawOptionValue::Bool(true));
        raw.insert("include_ast".to_string(), RawOptionValue::Bool(false));
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        // Should not panic; token-mode may or may not produce issues for this source.
        let _ = run_duplicate_block_with_options(&check, &make_duplicate_source(), &opts);
    }

    // ─── DuplicateBlock literal normalization (CD-331) ─────────────────────
    //
    // The ticket claimed identifiers were missed by canonicalisation; that
    // was already handled (window-relative local indices). The real gap:
    // `HashOp::Str`/`HashOp::Num` were hashed by value, so blocks differing
    // only in literals never grouped. These tests pin the fix at both the
    // `hash_ops` level (via `AstHashWalker::finish`) and the full
    // run+finalize level.

    #[test]
    fn ast_hash_string_literal_normalizes_but_exact_differs() {
        let a = r#"const label = "hello"; return label;"#;
        let b = r#"const label = "goodbye"; return label;"#;
        let (norm_a, exact_a) = ast_hash_first_n_stmts(a, 2);
        let (norm_b, exact_b) = ast_hash_first_n_stmts(b, 2);
        assert_eq!(
            norm_a, norm_b,
            "normalized hash should treat string literals as positional placeholders"
        );
        assert_ne!(
            exact_a, exact_b,
            "exact hash must still distinguish different string literal values"
        );
    }

    #[test]
    fn ast_hash_numeric_literal_normalizes_but_exact_differs() {
        let a = "const n = 1; return n;";
        let b = "const n = 2; return n;";
        let (norm_a, exact_a) = ast_hash_first_n_stmts(a, 2);
        let (norm_b, exact_b) = ast_hash_first_n_stmts(b, 2);
        assert_eq!(
            norm_a, norm_b,
            "normalized hash should treat numeric literals as positional placeholders"
        );
        assert_ne!(
            exact_a, exact_b,
            "exact hash must still distinguish different numeric literal values"
        );
    }

    #[test]
    fn ast_hash_normalization_does_not_collapse_genuinely_different_structure() {
        // Guard against over-normalising: differing operators are a
        // structural difference (`Bin:` tag payload), not a literal, and
        // must still hash differently even with literal normalization on.
        let a = "if (x === 1) return;";
        let b = "if (x !== 1) return;";
        let (norm_a, _) = ast_hash_first_n_stmts(a, 1);
        let (norm_b, _) = ast_hash_first_n_stmts(b, 1);
        assert_ne!(
            norm_a, norm_b,
            "different operators must still hash differently under literal normalization"
        );
    }

    #[test]
    fn ast_hash_string_placeholder_does_not_collide_with_numeric_placeholder() {
        // Both literals occupy local placeholder index 0 within their
        // window; distinct `Str#`/`Num#` prefixes must keep them from
        // aliasing each other.
        let with_string = r#"const first = "x";"#;
        let with_number = "const first = 1;";
        let (norm_str, _) = ast_hash_first_n_stmts(with_string, 1);
        let (norm_num, _) = ast_hash_first_n_stmts(with_number, 1);
        assert_ne!(
            norm_str, norm_num,
            "a string literal placeholder at index 0 must not hash the same as \
             a numeric literal placeholder at index 0"
        );
    }

    /// Run NearDuplicateBlock on two *different* sources (one per file)
    /// and return every claimed group's issue (see
    /// `finalize_all_groups_for_test`). Unlike
    /// `run_duplicate_block_with_options`, the two files needn't be
    /// byte-identical — used to test literal-only differences.
    fn run_duplicate_block_two_sources(
        check: &NearDuplicateBlock,
        source_a: &str,
        source_b: &str,
        options: &CheckOptions,
    ) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::default();

        let alloc_a = Allocator::default();
        let file_a = SourceFile::new(PathBuf::from("a.ts"), source_a.to_string());
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
        let file_b = SourceFile::new(PathBuf::from("b.ts"), source_b.to_string());
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
        finalize_all_groups_for_test(&mut finalize_ctx)
    }

    /// Six structurally identical statements whose literal values are
    /// parameterised, large enough to clear the default thresholds.
    fn make_duplicate_source_with_literals(product: &str, amount_cents: i64) -> String {
        format!(
            "const productId = \"{product}\";\n\
             const amountCents = {amount_cents};\n\
             const description = \"description for {product}\";\n\
             const invoice = createInvoice(productId, amountCents);\n\
             const receipt = submitInvoice(invoice, description);\n\
             return receipt;"
        )
    }

    /// Runs `NearDuplicateBlock::run` (the sole corpus writer) on two
    /// files, then finalizes against the same corpus twice: once via
    /// `finalize_all_groups_for_test` (every claimed group, exact clones
    /// included — what `Refactor.DuplicateBlock` used to report before
    /// it was cut in CD-357 pass 2) and once via the real
    /// `NearDuplicateBlock::finalize` (literal-drift groups only).
    /// Returns `(all_issues, near_issues)`. Used everywhere the CD-331
    /// split needs pinning: which half a group lands in, and that the
    /// two never claim overlapping territory.
    fn run_both_checks_two_sources(
        check: &NearDuplicateBlock,
        source_a: &str,
        source_b: &str,
        options: &CheckOptions,
    ) -> (Vec<CoreIssue>, Vec<CoreIssue>) {
        let corpus = CorpusIndex::default();

        let alloc_a = Allocator::default();
        let file_a = SourceFile::new(PathBuf::from("a.ts"), source_a.to_string());
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
        let file_b = SourceFile::new(PathBuf::from("b.ts"), source_b.to_string());
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

        let mut finalize_ctx_all = FinalizeContext::new(&corpus);
        let all_issues = finalize_all_groups_for_test(&mut finalize_ctx_all);
        let mut finalize_ctx_near = FinalizeContext::new(&corpus);
        let near_issues = check.finalize(&mut finalize_ctx_near);
        (all_issues, near_issues)
    }

    #[test]
    fn duplicate_block_exact_clone_is_collected_but_not_reported_by_near() {
        // Refactor.DuplicateBlock (the exact-clone half of the CD-331
        // split) was cut in CD-357 pass 2 — a verbatim clone is still
        // collected into the shared corpus slot (for the windowing/dedup
        // pass) but is no longer surfaced by any check.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source = make_duplicate_source();
        let (all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source, &source, &opts);
        assert_eq!(
            all_issues.len(),
            1,
            "expected exactly one claimed group for a verbatim clone, got {:?}",
            all_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        assert!(
            near_issues.is_empty(),
            "a verbatim clone must not be reported by Refactor.NearDuplicateBlock, got {:?}",
            near_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_literal_drift_lands_on_near_duplicate_block() {
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_duplicate_source_with_literals("gold", 4999);
        let source_b = make_duplicate_source_with_literals("silver", 2999);
        let (_all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source_a, &source_b, &opts);
        assert_eq!(
            near_issues.len(),
            1,
            "expected exactly one Refactor.NearDuplicateBlock finding for blocks differing \
             only in literals, got {:?}",
            near_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        assert_eq!(near_issues[0].check_id, "Refactor.NearDuplicateBlock");
        assert!(
            near_issues[0]
                .message
                .contains("differing only in literal values"),
            "message should call out that the blocks differ only in literal values, got {:?}",
            near_issues[0].message
        );
    }

    #[test]
    fn duplicate_block_normalize_literals_false_reproduces_old_grouping() {
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert(
            "normalize_literals".to_string(),
            RawOptionValue::Bool(false),
        );
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();
        let source_a = make_duplicate_source_with_literals("gold", 4999);
        let source_b = make_duplicate_source_with_literals("silver", 2999);
        let (all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source_a, &source_b, &opts);
        assert!(
            all_issues.is_empty() && near_issues.is_empty(),
            "with normalize_literals=false, blocks differing only in literal values must not \
             be grouped at all (pre-CD-331 behaviour), got all={:?} near={:?}",
            all_issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
            near_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_exact_and_near_groups_never_overlap_spans() {
        // Mix a verbatim-clone pair with a literal-drift pair across the
        // same two files and assert that no span (primary or related)
        // reported by the near-only check overlaps a span claimed by an
        // exact-clone group — pinning that `partition_claimed_groups`
        // runs its overlap-claim pass once, across every candidate group
        // together, rather than the exact and near halves claiming
        // independently.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let exact = make_duplicate_source();
        let near_a = make_duplicate_source_with_literals("gold", 4999);
        let near_b = make_duplicate_source_with_literals("silver", 2999);
        let source_a = format!("{exact}\n\n{near_a}");
        let source_b = format!("{exact}\n\n{near_b}");
        let (all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source_a, &source_b, &opts);
        assert_eq!(
            all_issues.len(),
            2,
            "expected the verbatim clone plus the literal-drift clone, got {all_issues:?}"
        );
        assert_eq!(
            near_issues.len(),
            1,
            "expected the literal-drift clone, got {near_issues:?}"
        );

        let spans_of = |issues: &[CoreIssue]| -> Vec<(PathBuf, u32, u32)> {
            issues
                .iter()
                .flat_map(|i| {
                    std::iter::once((i.file.clone(), i.location.line(), i.location.line())).chain(
                        i.related
                            .iter()
                            .map(|r| (r.file.clone(), r.location.line(), r.location.line())),
                    )
                })
                .collect()
        };
        let exact_issues: Vec<CoreIssue> = all_issues
            .into_iter()
            .filter(|i| {
                !near_issues
                    .iter()
                    .any(|n| n.location.line() == i.location.line() && n.file == i.file)
            })
            .collect();
        let exact_spans = spans_of(&exact_issues);
        let near_spans = spans_of(&near_issues);
        for (efile, eline, _) in &exact_spans {
            for (nfile, nline, _) in &near_spans {
                assert!(
                    !(efile == nfile && eline == nline),
                    "the exact-clone group and Refactor.NearDuplicateBlock reported the same \
                     location {efile:?}:{eline}"
                );
            }
        }
    }

    // ─── DuplicateBlock: import/re-export windows excluded (CD-331 follow-up) ──
    //
    // Real-repo validation of the literal-normalization fix above turned up a
    // false-positive class it did not anticipate: normalizing string literals
    // also normalizes module specifiers, so any two same-length runs of
    // `import` statements now hash equal — an import block can never be
    // "extracted into a shared helper", so the finding is never actionable.
    // Import declarations and re-exports (`export { x } from './y'`) are now
    // excluded from AST-mode windows entirely; a plain `export { x }` or
    // `export function f() {}` (no `source`) is ordinary code and still
    // participates.

    fn make_import_block(prefix: &str) -> String {
        (0..6)
            .map(|i| format!("import {{ value{i} }} from \"{prefix}/mod{i}\";"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn make_reexport_block(prefix: &str) -> String {
        (0..6)
            .map(|i| format!("export {{ value{i} }} from \"{prefix}/mod{i}\";"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn make_plain_export_function_block(offset: i64) -> String {
        (0..6)
            .map(|i| format!("export function step{i}() {{ return {}; }}", offset + i))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn duplicate_block_import_run_produces_no_finding() {
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_import_block("./a");
        let source_b = make_import_block("./b");
        let issues = run_duplicate_block_two_sources(&check, &source_a, &source_b, &opts);
        assert!(
            issues.is_empty(),
            "a run of six or more import declarations differing only in module \
             specifiers must not be flagged as a duplicate block, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_reexport_run_produces_no_finding() {
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_reexport_block("./a");
        let source_b = make_reexport_block("./b");
        let issues = run_duplicate_block_two_sources(&check, &source_a, &source_b, &opts);
        assert!(
            issues.is_empty(),
            "a run of re-exports (`export {{ x }} from './y'`) differing only in \
             module specifiers must not be flagged as a duplicate block, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_block_plain_export_function_still_participates_in_windows() {
        // Sanity guard against over-excluding: an `ExportNamedDeclaration`
        // with no `source` (a plain `export function`) is ordinary code and
        // must still be windowed and flagged like any other duplicate.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        // Same offset on both sides — a verbatim clone, so this stays a
        // pure windowing sanity check rather than exercising the
        // literal-drift split (that's covered separately, above).
        let source_a = make_plain_export_function_block(100);
        let source_b = make_plain_export_function_block(100);
        let issues = run_duplicate_block_two_sources(&check, &source_a, &source_b, &opts);
        assert_eq!(
            issues.len(),
            1,
            "a plain `export function` block with no `source` must still be \
             windowed and flagged, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    // ─── DuplicateBlock: run-scoped windowing + require blocks (CD-339) ────
    //
    // Four defects fixed together: (1) a window could straddle an excluded
    // statement, inflating its reported span with bytes it never hashed;
    // (2) `import x = require(...)` / `export = x` / `const x =
    // require(...)` were not excluded, so require blocks differing only in
    // module path false-positived as duplicates; (3) both messages
    // hardcoded `min_statements` instead of the effective per-file value
    // (covered indirectly — no dedicated test here, see `Fingerprint::
    // stmt_count`'s doc); (4) `NearDuplicateBlock`'s options were
    // advertised but never read (covered by the docs page, not a runtime
    // test).

    fn make_leader_trailer_source(with_imports: bool) -> String {
        // Two "leader" statements, optionally four side-effect imports,
        // then six "trailer" statements — imports sit between statement 2
        // and 3, splitting what a flattened-list windower would treat as
        // one run of 8 into a too-short leader run (2) and a full-length
        // trailer run (6).
        let leader = [
            "const leaderOne = computeLeader(1);",
            "const leaderTwo = computeLeader(2);",
        ];
        let imports = [
            "import \"./sideEffectOne\";",
            "import \"./sideEffectTwo\";",
            "import \"./sideEffectThree\";",
            "import \"./sideEffectFour\";",
        ];
        let trailer = [
            "const trailerZero = computeTrailer(10);",
            "const trailerOne = computeTrailer(11);",
            "const trailerTwo = computeTrailer(12);",
            "const trailerThree = computeTrailer(13);",
            "const trailerFour = computeTrailer(14);",
            "const trailerFive = computeTrailer(15);",
        ];
        let mut lines: Vec<&str> = leader.to_vec();
        if with_imports {
            lines.extend_from_slice(&imports);
        }
        lines.extend_from_slice(&trailer);
        lines.join("\n")
    }

    #[test]
    fn duplicate_block_window_does_not_straddle_excluded_statement() {
        // Identical leader+trailer statement sequences in both files; only
        // `source_a` has the four side-effect imports interleaved between
        // the leader and trailer runs. Both files report a duplicate for
        // the matched window, and the byte length of that window's span
        // must be identical in both files — the with-imports file's span
        // must not swallow the excluded imports' bytes.
        let check = NearDuplicateBlock::default();
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("min_chars".to_string(), RawOptionValue::Int(1));
        let opts =
            validate_options("Refactor.NearDuplicateBlock", DUP_BLOCK_OPTIONS, &raw).unwrap();

        let source_a = make_leader_trailer_source(true);
        let source_b = make_leader_trailer_source(false);
        let issues = run_duplicate_block_two_sources(&check, &source_a, &source_b, &opts);

        assert_eq!(
            issues.len(),
            1,
            "expected exactly one duplicate group, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        let issue = &issues[0];
        assert_eq!(issue.related.len(), 1, "expected exactly one related span");
        let primary_len = issue.location.end_byte() - issue.location.start_byte();
        let related_len =
            issue.related[0].location.end_byte() - issue.related[0].location.start_byte();
        assert_eq!(
            primary_len, related_len,
            "a window must not straddle the excluded import statements sitting between the \
             leader and trailer runs — the reported span must cover exactly the matched \
             statements in both files, not swallow the import block's bytes in the file that \
             has one"
        );
    }

    fn make_require_block(prefix: &str) -> String {
        (0..6)
            .map(|i| format!("const mod{i} = require(\"{prefix}/mod{i}\");"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn duplicate_block_const_require_block_produces_no_finding() {
        // Distinct module paths only, but otherwise structurally identical
        // statements: pre-fix, this doesn't land on `Refactor.DuplicateBlock`
        // (module-path strings differ, so `exact_hash` differs too) — it
        // lands on `Refactor.NearDuplicateBlock` instead, since
        // `normalize_literals` treats the path strings as positional
        // placeholders. Neither check should report a require block at all.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_require_block("./a");
        let source_b = make_require_block("./b");
        let (all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source_a, &source_b, &opts);
        assert!(
            all_issues.is_empty() && near_issues.is_empty(),
            "a run of six or more `const x = require(...)` declarations differing only in \
             module specifiers must not be flagged as a duplicate block, got all={:?} near={:?}",
            all_issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
            near_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    fn make_import_equals_block(prefix: &str) -> String {
        (0..6)
            .map(|i| format!("import mod{i} = require(\"{prefix}/mod{i}\");"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn duplicate_block_import_equals_require_block_produces_no_finding() {
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_import_equals_block("./a");
        let source_b = make_import_equals_block("./b");
        let (all_issues, near_issues) =
            run_both_checks_two_sources(&check, &source_a, &source_b, &opts);
        assert!(
            all_issues.is_empty() && near_issues.is_empty(),
            "a run of six or more `import x = require(...)` declarations differing only in \
             module specifiers must not be flagged as a duplicate block, got all={:?} near={:?}",
            all_issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
            near_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    fn make_plain_variable_block(offset: i64) -> String {
        (0..6)
            .map(|i| format!("const value{i} = {};", offset + i))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn duplicate_block_plain_variable_declarations_still_participate_in_windows() {
        // Regression guard for the require-block exclusion: a
        // `VariableDeclaration` whose initialisers are NOT `require(...)`
        // calls is ordinary code and must still be windowed and flagged —
        // this would fail if the require fix over-excluded every
        // `VariableDeclaration` instead of only require blocks.
        let check = NearDuplicateBlock::default();
        let opts = CheckOptions::defaults_from(DUP_BLOCK_OPTIONS);
        let source_a = make_plain_variable_block(100);
        let source_b = make_plain_variable_block(100);
        let issues = run_duplicate_block_two_sources(&check, &source_a, &source_b, &opts);
        assert_eq!(
            issues.len(),
            1,
            "ordinary variable declarations (not require blocks) must still be windowed and \
             flagged, got {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
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
