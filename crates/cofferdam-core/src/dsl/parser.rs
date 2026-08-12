//! Recursive-descent Pratt parser for the v1 predicate DSL.
//!
//! Entry points:
//! - [`parse_top`] — parse a full `top_predicate` (the value of a `when` /
//!   `require` field in `cofferdam.invariants.toml`).
//! - [`parse_predicate`] — parse a bare `predicate` (used internally and in
//!   tests).
//!
//! No parser-combinator libraries are used; this is hand-written recursive
//! descent with a precedence hierarchy. Operator precedence (low → high):
//!   or → and → not → atom
//!
//! Within `operand`, `+` (concat) is left-associative.
//!
//! All public errors carry 1-based line/column positions so config-load
//! diagnostics point at the right location in `cofferdam.invariants.toml`.

use crate::dsl::ast::{
    Call, Comparison, Op, Operand, Predicate, Subject, TopPredicate, KNOWN_BARE_SUBJECTS,
    KNOWN_FUNCTIONS, KNOWN_NAMESPACES, KNOWN_OPERATORS,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Parse errors produced by the DSL parser.
///
/// Every variant carries enough information for a human-readable, actionable
/// diagnostic. Line and column numbers are 1-based.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DslParseError {
    /// Unexpected or unrecognised token.
    #[error("unexpected token '{token}' at line {line}, col {col}: {msg}")]
    MalformedToken {
        line: u32,
        col: u32,
        token: String,
        msg: &'static str,
    },

    /// End of input reached before the predicate was complete.
    #[error("unexpected end of input: {msg}")]
    UnexpectedEof { msg: &'static str },

    /// An identifier used as a subject is not in the known-subjects vocabulary.
    #[error("unknown subject '{name}'{}", format_suggestions(.suggestions))]
    UnknownSubject {
        name: String,
        suggestions: Vec<String>,
    },

    /// An operator keyword is not recognised.
    #[error("unknown operator '{name}'{}", format_suggestions(.suggestions))]
    UnknownOperator {
        name: String,
        suggestions: Vec<String>,
    },

    /// A dotted namespace prefix in a subject is not in [`KNOWN_NAMESPACES`].
    #[error("subject '{subject}' uses unregistered namespace '{ns}'; known namespaces: {known}")]
    UnregisteredNamespace {
        subject: String,
        ns: String,
        known: String,
    },

    /// An invalid escape sequence inside a string literal.
    #[error("invalid escape \\{escape_char} in string literal at line {line}, col {col}")]
    BadStringEscape {
        line: u32,
        col: u32,
        escape_char: char,
    },

    /// The predicate nested (parens / calls / `+` concatenation) more deeply
    /// than [`MAX_PREDICATE_DEPTH`] allows. Guards against a stack overflow
    /// on adversarial input (CD-206) — a well-formed predicate never needs
    /// anywhere near this much nesting.
    #[error("predicate nested too deeply (limit {limit}) at line {line}, col {col}")]
    MaxDepthExceeded { line: u32, col: u32, limit: usize },
}

fn format_suggestions(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        String::new()
    } else {
        format!(" — did you mean '{}'?", suggestions.join("', '"))
    }
}

// ---------------------------------------------------------------------------
// Tokeniser
// ---------------------------------------------------------------------------

/// A lexical token produced by the tokeniser.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// An identifier, possibly dotted: `file`, `file.path`, `core.symbol`.
    Ident(String),
    /// A string literal (already unescaped).
    String(String),
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `+`
    Plus,
    /// `,`
    Comma,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
}

/// A token together with its position in the source string.
#[derive(Debug, Clone)]
struct Tok {
    token: Token,
    line: u32,
    col: u32,
}

/// Tokenise `src` into a `Vec<Tok>`.
///
/// Returns a `DslParseError` on the first bad character or bad string escape.
fn tokenise(src: &str) -> Result<Vec<Tok>, DslParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line: u32 = 1;
    let mut col: u32 = 1;

    macro_rules! advance {
        () => {{
            let c = chars[i];
            i += 1;
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            c
        }};
    }

    while i < chars.len() {
        let c = chars[i];
        let tok_line = line;
        let tok_col = col;

        match c {
            // Whitespace — skip.
            ' ' | '\t' | '\r' | '\n' => {
                advance!();
            }

            // Single-character tokens.
            '(' => {
                advance!();
                tokens.push(Tok {
                    token: Token::LParen,
                    line: tok_line,
                    col: tok_col,
                });
            }
            ')' => {
                advance!();
                tokens.push(Tok {
                    token: Token::RParen,
                    line: tok_line,
                    col: tok_col,
                });
            }
            '+' => {
                advance!();
                tokens.push(Tok {
                    token: Token::Plus,
                    line: tok_line,
                    col: tok_col,
                });
            }
            ',' => {
                advance!();
                tokens.push(Tok {
                    token: Token::Comma,
                    line: tok_line,
                    col: tok_col,
                });
            }

            // Two-character operators: `==`, `!=`.
            '=' => {
                advance!();
                if i < chars.len() && chars[i] == '=' {
                    advance!();
                    tokens.push(Tok {
                        token: Token::EqEq,
                        line: tok_line,
                        col: tok_col,
                    });
                } else {
                    return Err(DslParseError::MalformedToken {
                        line: tok_line,
                        col: tok_col,
                        token: "=".to_string(),
                        msg: "expected '==' (two equals signs)",
                    });
                }
            }
            '!' => {
                advance!();
                if i < chars.len() && chars[i] == '=' {
                    advance!();
                    tokens.push(Tok {
                        token: Token::BangEq,
                        line: tok_line,
                        col: tok_col,
                    });
                } else {
                    return Err(DslParseError::MalformedToken {
                        line: tok_line,
                        col: tok_col,
                        token: "!".to_string(),
                        msg: "expected '!=' (bang equals)",
                    });
                }
            }

            // String literals: single- or double-quoted.
            '\'' | '"' => {
                let quote = c;
                advance!(); // consume opening quote
                let mut s = String::new();
                let str_line = tok_line;
                let str_col = tok_col;
                loop {
                    if i >= chars.len() {
                        return Err(DslParseError::MalformedToken {
                            line: str_line,
                            col: str_col,
                            token: quote.to_string(),
                            msg: "unterminated string literal",
                        });
                    }
                    let ch = chars[i];
                    let esc_line = line;
                    let esc_col = col;
                    if ch == '\\' {
                        advance!(); // consume backslash
                        if i >= chars.len() {
                            return Err(DslParseError::MalformedToken {
                                line: esc_line,
                                col: esc_col,
                                token: "\\".to_string(),
                                msg: "trailing backslash in string literal",
                            });
                        }
                        let esc = chars[i];
                        advance!(); // consume escape char
                        match esc {
                            '\\' => s.push('\\'),
                            '\'' => s.push('\''),
                            '"' => s.push('"'),
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            other => {
                                return Err(DslParseError::BadStringEscape {
                                    line: esc_line,
                                    col: esc_col,
                                    escape_char: other,
                                });
                            }
                        }
                    } else if ch == quote {
                        advance!(); // consume closing quote
                        break;
                    } else {
                        s.push(ch);
                        advance!();
                    }
                }
                tokens.push(Tok {
                    token: Token::String(s),
                    line: str_line,
                    col: str_col,
                });
            }

            // Identifiers (ASCII letter or `_` start; letters, digits, `_`, `.` continue).
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut ident = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                        ident.push(ch);
                        advance!();
                    } else {
                        break;
                    }
                }
                tokens.push(Tok {
                    token: Token::Ident(ident),
                    line: tok_line,
                    col: tok_col,
                });
            }

            other => {
                return Err(DslParseError::MalformedToken {
                    line: tok_line,
                    col: tok_col,
                    token: other.to_string(),
                    msg: "unexpected character",
                });
            }
        }
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Parser state
// ---------------------------------------------------------------------------

/// Maximum predicate/operand nesting depth. Bounds both the parser's own
/// recursion (parens, `and`/`or`/`not`, call arguments) and, transitively,
/// the depth of the AST it produces — which in turn bounds the evaluator's
/// recursion over that AST (CD-206). Generous for any realistic predicate,
/// tight enough to never approach a stack overflow.
const MAX_PREDICATE_DEPTH: usize = 100;

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn new(tokens: Vec<Tok>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
        }
    }

    /// Enter one level of predicate nesting, erroring once
    /// [`MAX_PREDICATE_DEPTH`] is exceeded. Pair with [`Parser::leave_predicate`].
    fn enter_predicate(&mut self) -> Result<(), DslParseError> {
        self.depth += 1;
        if self.depth > MAX_PREDICATE_DEPTH {
            let (line, col) = self.current_pos();
            return Err(DslParseError::MaxDepthExceeded {
                line,
                col,
                limit: MAX_PREDICATE_DEPTH,
            });
        }
        Ok(())
    }

    /// Leave one level of predicate nesting entered via
    /// [`Parser::enter_predicate`]. Only call after a successful `enter_predicate`.
    fn leave_predicate(&mut self) {
        self.depth -= 1;
    }

    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.peek().map(|t| &t.token)
    }

    /// Return the current line/col for error reporting (1-based).
    /// Falls back to the last token's position when at EOF.
    fn current_pos(&self) -> (u32, u32) {
        if let Some(t) = self.tokens.get(self.pos) {
            (t.line, t.col)
        } else if let Some(t) = self.tokens.last() {
            (t.line, t.col)
        } else {
            (1, 1)
        }
    }

    fn consume(&mut self) -> Option<Tok> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    /// Expect an identifier token and return its string value.
    fn expect_ident(&mut self) -> Result<String, DslParseError> {
        match self.peek_token() {
            Some(Token::Ident(_)) => {
                let t = self.consume().unwrap();
                match t.token {
                    Token::Ident(s) => Ok(s),
                    _ => unreachable!(),
                }
            }
            Some(_) => {
                let (l, c) = self.current_pos();
                let t = self.consume().unwrap();
                Err(DslParseError::MalformedToken {
                    line: l,
                    col: c,
                    token: token_display(&t.token),
                    msg: "expected an identifier",
                })
            }
            None => Err(DslParseError::UnexpectedEof {
                msg: "expected an identifier",
            }),
        }
    }

    /// Expect `(`.
    fn expect_lparen(&mut self) -> Result<(), DslParseError> {
        match self.peek_token() {
            Some(Token::LParen) => {
                self.consume();
                Ok(())
            }
            Some(_) => {
                let (l, c) = self.current_pos();
                let t = self.consume().unwrap();
                Err(DslParseError::MalformedToken {
                    line: l,
                    col: c,
                    token: token_display(&t.token),
                    msg: "expected '('",
                })
            }
            None => Err(DslParseError::UnexpectedEof {
                msg: "expected '('",
            }),
        }
    }

    /// Expect `)`.
    fn expect_rparen(&mut self) -> Result<(), DslParseError> {
        match self.peek_token() {
            Some(Token::RParen) => {
                self.consume();
                Ok(())
            }
            Some(_) => {
                let (l, c) = self.current_pos();
                let t = self.consume().unwrap();
                Err(DslParseError::MalformedToken {
                    line: l,
                    col: c,
                    token: token_display(&t.token),
                    msg: "expected ')'",
                })
            }
            None => Err(DslParseError::UnexpectedEof {
                msg: "expected ')'",
            }),
        }
    }

    /// Check whether the next token is the identifier `kw` without consuming.
    fn peek_kw(&self, kw: &str) -> bool {
        matches!(self.peek_token(), Some(Token::Ident(s)) if s == kw)
    }

    /// Consume the next token if it is the identifier `kw`.
    fn try_consume_kw(&mut self, kw: &str) -> bool {
        if self.peek_kw(kw) {
            self.consume();
            true
        } else {
            false
        }
    }
}

fn token_display(t: &Token) -> String {
    match t {
        Token::Ident(s) => s.clone(),
        Token::String(s) => format!("'{s}'"),
        Token::LParen => "(".to_string(),
        Token::RParen => ")".to_string(),
        Token::Plus => "+".to_string(),
        Token::Comma => ",".to_string(),
        Token::EqEq => "==".to_string(),
        Token::BangEq => "!=".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Grammar rules
// ---------------------------------------------------------------------------

/// Parse a full `top_predicate` from `src`.
///
/// `top_predicate = ( "forbid" | "require" ) predicate | predicate`
pub fn parse_top(src: &str) -> Result<TopPredicate, DslParseError> {
    let tokens = tokenise(src)?;
    let mut p = Parser::new(tokens);
    let result = parse_top_predicate(&mut p)?;
    if let Some(tok) = p.peek() {
        return Err(DslParseError::MalformedToken {
            line: tok.line,
            col: tok.col,
            token: token_display(&tok.token),
            msg: "unexpected token after predicate",
        });
    }
    Ok(result)
}

/// Parse a bare `predicate` from `src`.
///
/// Useful in tests and for sub-expressions; the caller is responsible for
/// consuming any surrounding context.
pub fn parse_predicate(src: &str) -> Result<Predicate, DslParseError> {
    let tokens = tokenise(src)?;
    let mut p = Parser::new(tokens);
    let result = parse_or_expr(&mut p)?;
    if let Some(tok) = p.peek() {
        return Err(DslParseError::MalformedToken {
            line: tok.line,
            col: tok.col,
            token: token_display(&tok.token),
            msg: "unexpected token after predicate",
        });
    }
    Ok(result)
}

fn parse_top_predicate(p: &mut Parser) -> Result<TopPredicate, DslParseError> {
    if p.try_consume_kw("forbid") {
        let inner = parse_or_expr(p)?;
        Ok(TopPredicate::Forbid(Box::new(inner)))
    } else if p.try_consume_kw("require") {
        let inner = parse_or_expr(p)?;
        Ok(TopPredicate::Require(Box::new(inner)))
    } else {
        let inner = parse_or_expr(p)?;
        Ok(TopPredicate::Bare(Box::new(inner)))
    }
}

/// `or_expr = and_expr ( "or" and_expr )*`
///
/// This is the mutual-recursion hub for predicate nesting: parenthesised
/// atoms and call arguments both recurse back into `parse_or_expr`. Guarding
/// entry here bounds recursion depth for both paths (CD-206).
fn parse_or_expr(p: &mut Parser) -> Result<Predicate, DslParseError> {
    p.enter_predicate()?;
    let result = parse_or_expr_inner(p);
    p.leave_predicate();
    result
}

fn parse_or_expr_inner(p: &mut Parser) -> Result<Predicate, DslParseError> {
    let mut lhs = parse_and_expr(p)?;
    while p.try_consume_kw("or") {
        let rhs = parse_and_expr(p)?;
        lhs = Predicate::Or(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

/// `and_expr = not_expr ( "and" not_expr )*`
fn parse_and_expr(p: &mut Parser) -> Result<Predicate, DslParseError> {
    let mut lhs = parse_not_expr(p)?;
    while p.try_consume_kw("and") {
        let rhs = parse_not_expr(p)?;
        lhs = Predicate::And(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

/// `not_expr = [ "not" ] atom`
fn parse_not_expr(p: &mut Parser) -> Result<Predicate, DslParseError> {
    if p.try_consume_kw("not") {
        let inner = parse_atom(p)?;
        Ok(Predicate::Not(Box::new(inner)))
    } else {
        parse_atom(p)
    }
}

/// `atom = "(" predicate ")" | call | comparison`
///
/// Disambiguation rules (all via look-ahead on the next two tokens):
///
/// | Next token | Token after | Action |
/// |---|---|---|
/// | `(` | any | parenthesised sub-predicate |
/// | ident `+` `(` | any | call; if followed by op → subject of comparison |
/// | op keyword | string/call | implicit-file comparison (sugar from `forbid`/`require`) |
/// | known subject | op | comparison |
/// | known subject | `)` / `,` / `+` / EOF | bare subject used as value |
/// | other ident | `(` | call; if followed by op → subject of comparison |
/// | other ident | non-`(` | unknown subject error |
fn parse_atom(p: &mut Parser) -> Result<Predicate, DslParseError> {
    match p.peek_token() {
        Some(Token::LParen) => {
            p.consume(); // `(`
            let inner = parse_or_expr(p)?;
            p.expect_rparen()?;
            Ok(Predicate::Paren(Box::new(inner)))
        }
        Some(Token::String(_)) => {
            // A string literal (possibly part of a concat) used as a predicate
            // argument, e.g. `exists('tests/' + basename(file) + '.ts')`.
            // Parse as an `Operand` and wrap it in `Predicate::Operand`.
            let operand = parse_operand(p)?;
            Ok(Predicate::Operand(operand))
        }
        Some(Token::Ident(_)) => {
            let ident_pos = p.pos;
            let ident_tok = p.tokens[ident_pos].clone();
            let ident = match &ident_tok.token {
                Token::Ident(s) => s.clone(),
                _ => unreachable!(),
            };

            // Reject boolean/wrapper keywords in atom position.
            if is_boolean_kw(&ident) {
                let (l, c) = (ident_tok.line, ident_tok.col);
                return Err(DslParseError::MalformedToken {
                    line: l,
                    col: c,
                    token: ident.clone(),
                    msg: "unexpected boolean keyword in atom position",
                });
            }

            // Is the immediately following token a `(`?
            let next_is_lparen = matches!(
                p.tokens.get(ident_pos + 1),
                Some(Tok {
                    token: Token::LParen,
                    ..
                })
            );

            if next_is_lparen {
                // Parse as a call (function or namespaced subject call).
                let call = parse_call_inner(p)?;

                // If the next token is an operator, this call is the subject of
                // a comparison.
                if is_op_start(p) {
                    let subject = make_subject_from_call(call, &ident_tok)?;
                    let op = parse_op(p)?;
                    let operand = parse_operand(p)?;
                    Ok(Predicate::Comparison(Comparison {
                        subject,
                        op,
                        operand,
                    }))
                } else {
                    // Bare function call used as a boolean predicate (e.g. `exists(...)`).
                    Ok(Predicate::Call(call))
                }
            } else if is_op_keyword(&ident) {
                // The identifier is itself an operator keyword — this is the
                // `forbid imports 'X'` / `not imports 'X'` sugar where the
                // subject defaults to `file`.
                let implicit_subject = Subject::Identifier("file".to_string());
                let op = parse_op(p)?;
                let operand = parse_operand(p)?;
                Ok(Predicate::Comparison(Comparison {
                    subject: implicit_subject,
                    op,
                    operand,
                }))
            } else {
                // Identifier not followed by `(` and not an operator keyword.
                p.consume(); // consume the identifier

                // If the next token looks like an operator (or is an unknown
                // identifier that might be a mistyped operator), validate as
                // subject and build a comparison (so `parse_op` can produce an
                // `UnknownOperator` diagnostic).
                // "End of expression" markers (no operator expected):
                //   `)`, `,`, `+`, `or`, `and`, EOF.
                if is_comparison_rhs_start(p) {
                    let subject = validate_bare_subject(ident, &ident_tok)?;
                    let op = parse_op(p)?;
                    let operand = parse_operand(p)?;
                    Ok(Predicate::Comparison(Comparison {
                        subject,
                        op,
                        operand,
                    }))
                } else {
                    // Bare identifier used as a value reference (e.g. `basename(file)`,
                    // `core.symbol(X)` arg position).  Any identifier is valid here;
                    // the evaluator resolves it to the appropriate value at runtime.
                    Ok(Predicate::Call(Call {
                        name: ident,
                        args: vec![],
                    }))
                }
            }
        }
        Some(_) => {
            let (l, c) = p.current_pos();
            let t = p.consume().unwrap();
            Err(DslParseError::MalformedToken {
                line: l,
                col: c,
                token: token_display(&t.token),
                msg: "expected a comparison, function call, or '('",
            })
        }
        None => Err(DslParseError::UnexpectedEof {
            msg: "expected an atom (comparison, call, or parenthesised predicate)",
        }),
    }
}

fn is_boolean_kw(s: &str) -> bool {
    matches!(s, "or" | "and" | "not" | "forbid" | "require")
}

/// Returns `true` when the identifier is an operator keyword that can start a
/// comparison with an implicit `file` subject (after `forbid`/`require`/`not`).
fn is_op_keyword(s: &str) -> bool {
    matches!(s, "matches" | "imports" | "transitively" | "exports" | "in")
}

/// Returns `true` when the next token could be the start of a comparison
/// right-hand side (an operator, known or unknown).  Returns `false` when
/// the next token is an "end of expression" marker: `)`, `,`, `+`, `or`,
/// `and`, or EOF.
///
/// Used to decide whether a bare identifier should be the subject of a
/// comparison (and therefore parsed through `parse_op`) or a bare value
/// reference (no operator expected).
fn is_comparison_rhs_start(p: &Parser) -> bool {
    match p.peek_token() {
        // Known symbolic operators.
        Some(Token::EqEq) | Some(Token::BangEq) => true,
        // Any identifier that is NOT a boolean/wrapper keyword or an
        // end-of-expression marker is treated as an (possibly unknown) operator.
        Some(Token::Ident(s)) => !matches!(s.as_str(), "or" | "and" | "not" | "forbid" | "require"),
        // End-of-expression markers.
        Some(Token::RParen) | Some(Token::Comma) | Some(Token::Plus) => false,
        Some(Token::String(_)) | Some(Token::LParen) => false,
        None => false,
    }
}

/// Returns `true` when the current token is the start of an operator.
fn is_op_start(p: &Parser) -> bool {
    match p.peek_token() {
        Some(Token::EqEq) | Some(Token::BangEq) => true,
        Some(Token::Ident(s)) => matches!(
            s.as_str(),
            "matches" | "imports" | "transitively" | "exports" | "in"
        ),
        _ => false,
    }
}

/// Parse a `call` production: `identifier "(" [ args ] ")"`.
///
/// The identifier has already been identified (via peek) as the next token.
fn parse_call_inner(p: &mut Parser) -> Result<Call, DslParseError> {
    let name = p.expect_ident()?;
    p.expect_lparen()?;
    let mut args = Vec::new();
    if !matches!(p.peek_token(), Some(Token::RParen)) {
        let first = parse_or_expr(p)?;
        args.push(first);
        while matches!(p.peek_token(), Some(Token::Comma)) {
            p.consume(); // `,`
            let arg = parse_or_expr(p)?;
            args.push(arg);
        }
    }
    p.expect_rparen()?;
    Ok(Call { name, args })
}

/// Convert a `Call` into a `Subject::Call`, validating the namespace prefix.
fn make_subject_from_call(call: Call, tok: &Tok) -> Result<Subject, DslParseError> {
    validate_call_namespace(&call.name, tok)?;
    Ok(Subject::Call(call))
}

/// Validate that the dotted namespace prefix (if present) is in
/// `KNOWN_NAMESPACES`. Bare identifiers and known bare-subject dotted names
/// (`file.path`, `file.layer`) pass without a namespace check.
fn validate_call_namespace(name: &str, _tok: &Tok) -> Result<(), DslParseError> {
    if let Some(dot_pos) = name.find('.') {
        let ns = &name[..dot_pos];
        if !KNOWN_NAMESPACES.contains(&ns) {
            return Err(DslParseError::UnregisteredNamespace {
                subject: name.to_string(),
                ns: ns.to_string(),
                known: KNOWN_NAMESPACES.join(", "),
            });
        }
    } else {
        // No dot — must be a known function name.
        if !KNOWN_FUNCTIONS.contains(&name) {
            let suggestions: Vec<String> = levenshtein_suggestions(name, KNOWN_FUNCTIONS)
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            return Err(DslParseError::UnknownSubject {
                name: name.to_string(),
                suggestions,
            });
        }
        // Known functions are fine here; namespace check not applicable.
    }
    Ok(())
}

/// Validate a bare (non-call) subject identifier.
fn validate_bare_subject(name: String, _tok: &Tok) -> Result<Subject, DslParseError> {
    // Known bare subjects: `file`, `file.path`, `file.layer`.
    if KNOWN_BARE_SUBJECTS.contains(&name.as_str()) {
        return Ok(Subject::Identifier(name));
    }

    // Dotted names not in the bare list: check namespace.
    if let Some(dot_pos) = name.find('.') {
        let ns = &name[..dot_pos];
        if !KNOWN_NAMESPACES.contains(&ns) {
            return Err(DslParseError::UnregisteredNamespace {
                subject: name.clone(),
                ns: ns.to_string(),
                known: KNOWN_NAMESPACES.join(", "),
            });
        }
        // Known namespace but not a call — still represent as identifier.
        return Ok(Subject::Identifier(name));
    }

    // Unknown bare identifier — suggest alternatives.
    let suggestions = levenshtein_suggestions(&name, KNOWN_BARE_SUBJECTS);
    Err(DslParseError::UnknownSubject {
        name,
        suggestions: suggestions.into_iter().map(|s| s.to_string()).collect(),
    })
}

/// Parse an operator.
///
/// Handles multi-word operators:
///   `transitively imports`, `imports as type`, `imports as value`.
fn parse_op(p: &mut Parser) -> Result<Op, DslParseError> {
    match p.peek_token() {
        Some(Token::EqEq) => {
            p.consume();
            Ok(Op::Eq)
        }
        Some(Token::BangEq) => {
            p.consume();
            Ok(Op::Ne)
        }
        Some(Token::Ident(s)) => {
            let s = s.clone();
            match s.as_str() {
                "matches" => {
                    p.consume();
                    Ok(Op::Matches)
                }
                "exports" => {
                    p.consume();
                    Ok(Op::Exports)
                }
                "in" => {
                    p.consume();
                    Ok(Op::In)
                }
                "imports" => {
                    p.consume();
                    // Look ahead for `as type` / `as value`.
                    if p.try_consume_kw("as") {
                        if p.try_consume_kw("type") {
                            Ok(Op::ImportsAsType)
                        } else if p.try_consume_kw("value") {
                            Ok(Op::ImportsAsValue)
                        } else {
                            let (l, c) = p.current_pos();
                            Err(DslParseError::MalformedToken {
                                line: l,
                                col: c,
                                token: p
                                    .peek()
                                    .map(|t| token_display(&t.token))
                                    .unwrap_or_else(|| "<eof>".to_string()),
                                msg: "expected 'type' or 'value' after 'imports as'",
                            })
                        }
                    } else {
                        Ok(Op::Imports)
                    }
                }
                "transitively" => {
                    p.consume();
                    if p.try_consume_kw("imports") {
                        Ok(Op::TransitivelyImports)
                    } else {
                        let (l, c) = p.current_pos();
                        Err(DslParseError::MalformedToken {
                            line: l,
                            col: c,
                            token: p
                                .peek()
                                .map(|t| token_display(&t.token))
                                .unwrap_or_else(|| "<eof>".to_string()),
                            msg: "expected 'imports' after 'transitively'",
                        })
                    }
                }
                _ => {
                    // Unknown operator — suggest alternatives.
                    let suggestions = levenshtein_suggestions(&s, KNOWN_OPERATORS);
                    Err(DslParseError::UnknownOperator {
                        name: s,
                        suggestions: suggestions.into_iter().map(|s| s.to_string()).collect(),
                    })
                }
            }
        }
        Some(_) => {
            let (l, c) = p.current_pos();
            let t = p.consume().unwrap();
            Err(DslParseError::MalformedToken {
                line: l,
                col: c,
                token: token_display(&t.token),
                msg: "expected an operator",
            })
        }
        None => Err(DslParseError::UnexpectedEof {
            msg: "expected an operator after subject",
        }),
    }
}

/// `operand = string | call | concat`  (concat is left-associative `+`)
///
/// This loop is iterative, not recursive, so it doesn't hit
/// `Parser::enter_predicate`'s guard — but it still builds a left-nested
/// `Operand::Concat` tree that the evaluator recurses over. Bound the chain
/// length directly so a predicate with thousands of `+`s can't stack-overflow
/// evaluation later (CD-206).
fn parse_operand(p: &mut Parser) -> Result<Operand, DslParseError> {
    let mut lhs = parse_operand_atom(p)?;
    let mut concat_len: usize = 1;
    while matches!(p.peek_token(), Some(Token::Plus)) {
        p.consume(); // `+`
        let rhs = parse_operand_atom(p)?;
        lhs = Operand::Concat(Box::new(lhs), Box::new(rhs));
        concat_len += 1;
        if concat_len > MAX_PREDICATE_DEPTH {
            let (line, col) = p.current_pos();
            return Err(DslParseError::MaxDepthExceeded {
                line,
                col,
                limit: MAX_PREDICATE_DEPTH,
            });
        }
    }
    Ok(lhs)
}

/// Primary operand: `string | call`.
fn parse_operand_atom(p: &mut Parser) -> Result<Operand, DslParseError> {
    match p.peek_token() {
        Some(Token::String(_)) => {
            let t = p.consume().unwrap();
            match t.token {
                Token::String(s) => Ok(Operand::String(s)),
                _ => unreachable!(),
            }
        }
        Some(Token::Ident(_)) => {
            // Must be a function call.
            let ident_pos = p.pos;
            let next_is_lparen = matches!(
                p.tokens.get(ident_pos + 1),
                Some(Tok {
                    token: Token::LParen,
                    ..
                })
            );
            if next_is_lparen {
                let call = parse_call_inner(p)?;
                Ok(Operand::Call(call))
            } else {
                // Bare identifier as operand — treat as a string reference.
                // The grammar only lists `string | call | concat` for operand,
                // so a bare identifier without parens is not a valid operand.
                let (l, c) = p.current_pos();
                let t = p.consume().unwrap();
                Err(DslParseError::MalformedToken {
                    line: l,
                    col: c,
                    token: token_display(&t.token),
                    msg: "expected a string literal or function call as operand",
                })
            }
        }
        Some(_) => {
            let (l, c) = p.current_pos();
            let t = p.consume().unwrap();
            Err(DslParseError::MalformedToken {
                line: l,
                col: c,
                token: token_display(&t.token),
                msg: "expected a string literal or function call as operand",
            })
        }
        None => Err(DslParseError::UnexpectedEof {
            msg: "expected an operand",
        }),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute Levenshtein distance between two strings (byte-based).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Return candidates from `pool` within Levenshtein distance 2 of `name`,
/// sorted by distance. Returns at most two suggestions.
fn levenshtein_suggestions<'a>(name: &str, pool: &[&'a str]) -> Vec<&'a str> {
    let mut scored: Vec<(&str, usize)> = pool
        .iter()
        .filter_map(|&candidate| {
            let d = levenshtein(name, candidate);
            if d <= 2 {
                Some((candidate, d))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(_, d)| *d);
    scored.into_iter().map(|(s, _)| s).take(2).collect()
}

// ---------------------------------------------------------------------------
// Round-trip (unparse)
// ---------------------------------------------------------------------------

/// Convert a [`TopPredicate`] back to a canonical source string.
///
/// The output is a valid predicate string that the parser accepts; it may
/// not be byte-for-byte identical to the original input (whitespace and quote
/// style are normalised). Used in round-trip tests.
pub fn unparse_top(top: &TopPredicate) -> String {
    match top {
        TopPredicate::Forbid(p) => format!("forbid {}", unparse(p)),
        TopPredicate::Require(p) => format!("require {}", unparse(p)),
        TopPredicate::Bare(p) => unparse(p),
    }
}

/// Convert a [`Predicate`] back to a canonical source string.
pub fn unparse(pred: &Predicate) -> String {
    match pred {
        Predicate::Or(l, r) => format!("{} or {}", unparse_and(l), unparse_and(r)),
        Predicate::And(_, _) => unparse_and(pred),
        Predicate::Not(_) => unparse_not(pred),
        Predicate::Paren(inner) => format!("({})", unparse(inner)),
        Predicate::Comparison(c) => unparse_comparison(c),
        Predicate::Call(c) => unparse_call(c),
        Predicate::Operand(o) => unparse_operand(o),
    }
}

fn unparse_and(pred: &Predicate) -> String {
    match pred {
        Predicate::And(l, r) => format!("{} and {}", unparse_not(l), unparse_not(r)),
        other => unparse_not(other),
    }
}

fn unparse_not(pred: &Predicate) -> String {
    match pred {
        Predicate::Not(inner) => format!("not {}", unparse_atom(inner)),
        other => unparse_atom(other),
    }
}

fn unparse_atom(pred: &Predicate) -> String {
    match pred {
        Predicate::Paren(inner) => format!("({})", unparse(inner)),
        Predicate::Comparison(c) => unparse_comparison(c),
        Predicate::Call(c) => unparse_call(c),
        Predicate::Operand(o) => unparse_operand(o),
        // An Or/And/Not at atom level needs parenthesisation.
        other => format!("({})", unparse(other)),
    }
}

fn unparse_comparison(c: &Comparison) -> String {
    let subj = unparse_subject(&c.subject);
    let op = unparse_op(&c.op);
    let operand = unparse_operand(&c.operand);
    format!("{subj} {op} {operand}")
}

fn unparse_subject(s: &Subject) -> String {
    match s {
        Subject::Identifier(name) => name.clone(),
        Subject::Call(call) => unparse_call(call),
        Subject::Paren(inner) => format!("({})", unparse(inner)),
    }
}

fn unparse_op(op: &Op) -> String {
    match op {
        Op::Matches => "matches".to_string(),
        Op::Imports => "imports".to_string(),
        Op::TransitivelyImports => "transitively imports".to_string(),
        Op::ImportsAsType => "imports as type".to_string(),
        Op::ImportsAsValue => "imports as value".to_string(),
        Op::Exports => "exports".to_string(),
        Op::In => "in".to_string(),
        Op::Eq => "==".to_string(),
        Op::Ne => "!=".to_string(),
    }
}

fn unparse_operand(o: &Operand) -> String {
    match o {
        Operand::String(s) => format!("'{}'", s.replace('\'', "\\'")),
        Operand::Call(c) => unparse_call(c),
        Operand::Concat(l, r) => format!("{} + {}", unparse_operand(l), unparse_operand(r)),
    }
}

fn unparse_call(c: &Call) -> String {
    let args: Vec<String> = c.args.iter().map(unparse).collect();
    format!("{}({})", c.name, args.join(", "))
}
