//! AST types for the v1 predicate DSL.
//!
//! The shapes here follow the EBNF in `docs/dsl-grammar.md` exactly.
//! Reserved-for-v2 surfaces (quantifiers, aggregation, path-shape predicates)
//! are intentionally absent — add them through a MINOR bump once cd-9hp.9
//! ships the graph substrate.

/// Top-level predicate — the entry-point type produced by the parser.
///
/// Corresponds to `predicate = or_expr` in the grammar, but includes the
/// optional `top_predicate` sugar (`forbid` / `require` wrappers).
#[derive(Debug, Clone, PartialEq)]
pub enum TopPredicate {
    /// `forbid <predicate>` — sugar for `not <predicate>`.
    Forbid(Box<Predicate>),
    /// `require <predicate>` — sugar for `<predicate>` (identity wrapper;
    /// kept distinct so a future unparser / linter can round-trip it).
    Require(Box<Predicate>),
    /// Bare predicate with no top-level wrapper.
    Bare(Box<Predicate>),
}

/// Boolean predicate expression.
///
/// Mirrors `or_expr`, `and_expr`, `not_expr`, and `atom` from the grammar
/// collapsed into one enum to avoid deeply-nested wrapper structs.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// `a or b`
    Or(Box<Predicate>, Box<Predicate>),
    /// `a and b`
    And(Box<Predicate>, Box<Predicate>),
    /// `not a`
    Not(Box<Predicate>),
    /// `(predicate)` — parenthesised sub-expression. The inner `Predicate`
    /// is the unwrapped content; this variant is retained so the AST can
    /// faithfully represent what the user wrote (useful for linting and
    /// future round-trip serialisation). The evaluator ignores the wrapper.
    Paren(Box<Predicate>),
    /// `subject op operand` — a direct comparison.
    Comparison(Comparison),
    /// `identifier(args)` — a function call used as a boolean predicate.
    /// Only `exists(p)` returns `bool`; the others return strings and
    /// must appear as `operand`, not as a top-level predicate.  The
    /// grammar admits `call` in `atom`, so the parser lets a call stand as
    /// a predicate; the evaluator type-checks at evaluation time.
    Call(Call),
    /// A bare operand expression (string literal or concat) used as a
    /// predicate argument.  Arises in function call positions where the
    /// grammar says `args = predicate` but the user passes a string value
    /// (e.g. `exists('tests/' + basename(file) + '.ts')`).  The evaluator
    /// evaluates these as string values; only `exists(...)` consumes them.
    Operand(Operand),
}

/// `comparison = subject op operand`
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub subject: Subject,
    pub op: Op,
    pub operand: Operand,
}

/// Subject of a comparison — what the operator applies to.
///
/// `subject = identifier | "(" predicate ")"` in the grammar.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    /// Simple identifier (e.g. `file`, `file.path`, `file.layer`).
    Identifier(String),
    /// Namespaced function call used as a subject
    /// (e.g. `core.symbol(name)`, `ts.declaration(name)`).
    ///
    /// The namespace is the part before the first `.` in the identifier
    /// (e.g. `"core"` in `core.symbol`). The parser validates that the
    /// namespace is in [`KNOWN_NAMESPACES`] and returns
    /// [`crate::dsl::parser::DslParseError::UnregisteredNamespace`]
    /// otherwise.
    Call(Call),
    /// Parenthesised sub-predicate used as a subject.
    Paren(Box<Predicate>),
}

/// The eight operators from the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `matches` — gitignore-style glob match.
    Matches,
    /// `imports` — direct import edge.
    Imports,
    /// `transitively imports` — transitive-closure import.
    TransitivelyImports,
    /// `imports as type` — type-only import.
    ImportsAsType,
    /// `imports as value` — value import.
    ImportsAsValue,
    /// `exports` — file exports a named symbol.
    Exports,
    /// `in` — file is in a layer.
    In,
    /// `==` — string equality.
    Eq,
    /// `!=` — string inequality.
    Ne,
}

/// Operand of a comparison.
///
/// `operand = string | call | concat`  (`concat = operand "+" operand`)
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A string literal (already unescaped).
    String(String),
    /// A function call that yields a string (e.g. `basename(file)`).
    Call(Call),
    /// `left + right` — left-associative string concatenation.
    Concat(Box<Operand>, Box<Operand>),
}

/// A function call: `identifier "(" [ args ] ")"`
///
/// `args = predicate ( "," predicate )*`
///
/// In v1 `args` is actually `operand` in the positions where function calls
/// appear as operands (the three built-ins each take a single string operand
/// or a subject reference). The grammar says `predicate` in `args` for
/// maximum composability; the evaluator enforces the typed constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The fully-qualified name, e.g. `"basename"`, `"core.symbol"`.
    pub name: String,
    /// Positional arguments.
    pub args: Vec<Predicate>,
}

/// Namespaces that v1 recognises in subject identifiers.
///
/// The parser checks that any dotted prefix in a subject call is in this set
/// and returns [`crate::dsl::parser::DslParseError::UnregisteredNamespace`]
/// otherwise.
pub const KNOWN_NAMESPACES: &[&str] = &["core", "ts"];

/// Known subjects (bare identifiers, not calls).
pub const KNOWN_BARE_SUBJECTS: &[&str] = &["file", "file.path", "file.layer"];

/// All operator keyword tokens (used for typo suggestions in error messages).
pub const KNOWN_OPERATORS: &[&str] = &[
    "matches",
    "imports",
    "transitively",
    "exports",
    "in",
    "==",
    "!=",
];

/// All built-in function names.
pub const KNOWN_FUNCTIONS: &[&str] = &["basename", "dirname", "exists"];
