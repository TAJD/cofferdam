//! AST wire format builder (cd-svf).
//!
//! Per `design/sdk-ast-wire.md`: option D — flat array with child-offset
//! indices. Builds a JSON-serialisable representation of a parsed file's
//! AST that the Node plugin host can reconstruct into a typed `AstView`.
//!
//! v0 implementation emits the 9 v0 NodeKind variants (Program,
//! CallExpression, ImportDeclaration, Function, ArrowFunctionExpression,
//! Class, ObjectExpression, MemberExpression, IdentifierReference).
//! Non-v0 oxc nodes traverse silently — they don't appear in the wire,
//! and the parent stack stays anchored on the nearest v0 ancestor. This
//! is consistent with the design doc's "additive growth" rule: future
//! kinds slot in as new visit-method overrides without breaking the wire
//! format.
//!
//! Each emitted node carries:
//!   - `kind`: the canonical name (matches `NodeKind` in the SDK).
//!   - `span`: line/column + byte range, round-trips against source.
//!   - `firstChild` / `nextSibling`: indices into the flat array. -1 = end.
//!   - per-kind typed extras (calleeIdx, paramIdxs, source, name, ...).
//!
//! No `parentIdx` in v0 (cd-4de Q1 deferred). The parent is tracked
//! during build via a stack frame; not part of the public wire format.

use cofferdam_core::span_from_bytes;
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, CallExpression, Class, Expression, Function,
    ImportDeclaration, ImportDeclarationSpecifier, JSXAttribute, JSXAttributeName,
    JSXAttributeValue, JSXElement, JSXExpressionContainer, JSXSpreadAttribute, MemberExpression,
    ObjectExpression, ObjectPropertyKind, Program, VariableDeclaration,
};
use oxc_ast_visit::{walk, Visit};
use oxc_span::GetSpan;
use oxc_syntax::scope::ScopeFlags;
use serde::Serialize;

#[derive(Serialize)]
pub struct AstWire {
    #[serde(rename = "rootIdx")]
    pub root_idx: i32,
    pub nodes: Vec<WireNode>,
}

#[derive(Serialize)]
pub struct WireNode {
    pub kind: &'static str,
    pub span: cofferdam_core::Span,
    #[serde(rename = "firstChild")]
    pub first_child: i32,
    #[serde(rename = "nextSibling")]
    pub next_sibling: i32,
    #[serde(flatten)]
    pub extras: WireExtras,
}

/// Per-kind typed extras. `#[serde(untagged)]` flattens into the
/// containing object so the wire shape matches the SDK's typed
/// AstNode interfaces.
#[derive(Serialize)]
#[serde(untagged)]
pub enum WireExtras {
    None {},
    Call {
        #[serde(rename = "calleeIdx")]
        callee_idx: i32,
        #[serde(rename = "argumentIdxs")]
        argument_idxs: Vec<i32>,
    },
    Import {
        source: String,
        specifiers: Vec<ImportSpecifierWire>,
    },
    Function {
        name: Option<String>,
        #[serde(rename = "paramIdxs")]
        param_idxs: Vec<i32>,
        #[serde(rename = "async")]
        is_async: bool,
        generator: bool,
    },
    Arrow {
        #[serde(rename = "paramIdxs")]
        param_idxs: Vec<i32>,
        #[serde(rename = "async")]
        is_async: bool,
        expression: bool,
    },
    Class {
        name: Option<String>,
    },
    Object {
        #[serde(rename = "propertyIdxs")]
        property_idxs: Vec<i32>,
    },
    Member {
        #[serde(rename = "objectIdx")]
        object_idx: i32,
        property: Option<String>,
        computed: bool,
    },
    Identifier {
        name: String,
    },
    Element {
        #[serde(rename = "tagName")]
        tag_name: String,
        #[serde(rename = "selfClosing")]
        self_closing: bool,
        #[serde(rename = "attributeIdxs")]
        attribute_idxs: Vec<i32>,
    },
    Attribute {
        name: Option<String>,
        value: Option<String>,
        #[serde(rename = "isExpression")]
        is_expression: bool,
        #[serde(rename = "isSpread")]
        is_spread: bool,
    },
    /// CD-84: HTML `Text` node — the raw text content between tags.
    Text {
        text: String,
    },
    /// CD-84: HTML `Comment` node — raw comment text, delimiters
    /// (`<!--`/`-->`) included, matching `HtmlParseTree::text_of`.
    Comment {
        text: String,
    },
    /// CD-84: HTML `Doctype` node — raw doctype declaration text
    /// (e.g. `<!DOCTYPE html>`).
    Doctype {
        text: String,
    },
    /// CD-78: a `const`/`let`/`var` statement. One node per statement,
    /// covering every declarator. `declarations` mirrors
    /// `ImportDeclaration`'s inline sub-item list rather than emitting a
    /// separate `VariableDeclarator` kind. A declarator's `init_idx` is
    /// the flat-array index of its initializer expression when that
    /// initializer is itself a v0 node (e.g. an arrow/call), else -1.
    VariableDeclaration {
        #[serde(rename = "declarationKind")]
        declaration_kind: String,
        declarations: Vec<VariableDeclaratorWire>,
    },
}

#[derive(Serialize)]
pub struct ImportSpecifierWire {
    #[serde(rename = "localName")]
    pub local_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported: Option<String>,
}

#[derive(Serialize)]
pub struct VariableDeclaratorWire {
    /// The bound name for a simple `BindingIdentifier`; `None` for a
    /// destructuring pattern (`const { a } = ...`) — CD-78 leaves nested
    /// pattern-path resolution to a follow-up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "initIdx")]
    pub init_idx: i32,
    /// CD-190: the initializer's own span, populated whenever an
    /// initializer is present — independent of `init_idx`, which stays
    /// `-1` for initializer kinds the v0 wire surface doesn't model as a
    /// full node (string/numeric literals, binary expressions, ...).
    /// Lets a plugin isolate just the initializer's source text even
    /// when `init` itself is `undefined`.
    #[serde(rename = "initSpan", skip_serializing_if = "Option::is_none")]
    pub init_span: Option<cofferdam_core::Span>,
}

struct StackFrame {
    /// idx of the node whose children are currently being added.
    parent_idx: i32,
    /// idx of the last child added under this parent, or -1.
    last_child_idx: i32,
}

pub struct WireBuilder<'a> {
    text: &'a str,
    nodes: Vec<WireNode>,
    stack: Vec<StackFrame>,
}

impl<'a> WireBuilder<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            nodes: Vec::new(),
            stack: Vec::new(),
        }
    }

    pub fn build(mut self, program: &Program<'a>) -> AstWire {
        self.visit_program(program);
        let root_idx = if self.nodes.is_empty() { -1 } else { 0 };
        AstWire {
            root_idx,
            nodes: self.nodes,
        }
    }

    /// Push a new node into the flat array, link it as a sibling/child
    /// of the current parent, and return its index.
    fn push(&mut self, kind: &'static str, span: oxc_span::Span) -> i32 {
        let idx = self.nodes.len() as i32;
        let (parent_idx, prev_sibling) = match self.stack.last() {
            Some(f) => (f.parent_idx, f.last_child_idx),
            None => (-1, -1),
        };

        self.nodes.push(WireNode {
            kind,
            span: span_from_bytes(self.text, span.start, span.end),
            first_child: -1,
            next_sibling: -1,
            extras: WireExtras::None {},
        });

        if prev_sibling >= 0 {
            self.nodes[prev_sibling as usize].next_sibling = idx;
        } else if parent_idx >= 0 {
            self.nodes[parent_idx as usize].first_child = idx;
        }

        if let Some(f) = self.stack.last_mut() {
            f.last_child_idx = idx;
        }
        idx
    }

    fn enter(&mut self, idx: i32) {
        self.stack.push(StackFrame {
            parent_idx: idx,
            last_child_idx: -1,
        });
    }

    fn exit(&mut self) {
        self.stack.pop();
    }
}

impl<'a> Visit<'a> for WireBuilder<'a> {
    fn visit_program(&mut self, it: &Program<'a>) {
        let idx = self.push("Program", it.span);
        self.enter(idx);
        walk::walk_program(self, it);
        self.exit();
        // Program has no per-kind extras — body is reconstructed
        // host-side from the firstChild/nextSibling chain.
        let _ = idx;
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let idx = self.push("CallExpression", it.span);
        self.enter(idx);

        let callee_idx = self.nodes.len() as i32;
        self.visit_expression(&it.callee);
        // Skip type_arguments — not part of v0 surface; descend into
        // them only so non-v0 traversal keeps semantic context, but
        // they don't affect the calleeIdx/argumentIdxs split because
        // type_arguments produce no v0 emissions.
        if let Some(ty_args) = &it.type_arguments {
            self.visit_ts_type_parameter_instantiation(ty_args);
        }
        let mut argument_idxs = Vec::with_capacity(it.arguments.len());
        for arg in &it.arguments {
            let arg_span = arg.span();
            let arg_first_idx = self.nodes.len() as i32;
            self.visit_argument(arg);
            // Only record an argument idx if the visit pushed a node
            // that IS the argument itself — i.e. its span matches the
            // argument's own span exactly. A non-v0 argument kind (e.g.
            // TemplateLiteral) still gets descended into (so v0 nodes
            // nested inside it, like an identifier in `${...}`, are
            // still discoverable via find_all/walk), but the first node
            // that descent happens to push is a *descendant*, not a
            // representation of the argument — recording it here would
            // misattribute the descendant's (truncated) span as the
            // argument's own, which is exactly the cd-18 bug.
            if let Some(pushed) = self.nodes.get(arg_first_idx as usize) {
                if pushed.span.start_byte == arg_span.start && pushed.span.end_byte == arg_span.end
                {
                    argument_idxs.push(arg_first_idx);
                }
            }
        }

        self.exit();
        // The callee may have descended into a non-v0 expression that
        // didn't push anything. In that case callee_idx points past
        // the array and is invalid; clamp to -1.
        let callee_idx_final = if (callee_idx as usize) < self.nodes.len() {
            callee_idx
        } else {
            -1
        };
        self.nodes[idx as usize].extras = WireExtras::Call {
            callee_idx: callee_idx_final,
            argument_idxs,
        };
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        let idx = self.push("ImportDeclaration", it.span);
        let source = it.source.value.to_string();
        let mut specifiers = Vec::new();
        if let Some(specs) = &it.specifiers {
            for spec in specs {
                let s = match spec {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => ImportSpecifierWire {
                        local_name: s.local.name.to_string(),
                        imported: Some(s.imported.name().to_string()),
                    },
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => ImportSpecifierWire {
                        local_name: s.local.name.to_string(),
                        imported: Some("default".to_string()),
                    },
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        ImportSpecifierWire {
                            local_name: s.local.name.to_string(),
                            imported: Some("*".to_string()),
                        }
                    }
                };
                specifiers.push(s);
            }
        }
        self.nodes[idx as usize].extras = WireExtras::Import { source, specifiers };
        // Don't descend — ImportDeclaration's children (specifiers,
        // source StringLiteral) aren't v0 surface kinds. Leaving them
        // out keeps the wire small without affecting plugin queries.
    }

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        let idx = self.push("VariableDeclaration", it.span);
        self.enter(idx);

        let declaration_kind = it.kind.as_str().to_string();
        let mut declarations = Vec::with_capacity(it.declarations.len());
        for decl in &it.declarations {
            let name = match &decl.id {
                BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                _ => None,
            };
            // Serialize the initializer as a sub-expression, recording its
            // idx only when the first pushed node IS the initializer (its
            // span matches exactly). A non-v0 init (e.g. a numeric literal)
            // pushes nothing / a descendant, so it's left as -1 — the same
            // span-match guard visit_call_expression uses for its args.
            let mut init_idx = -1;
            let mut init_span = None;
            if let Some(init) = &decl.init {
                let raw_span = init.span();
                init_span = Some(span_from_bytes(self.text, raw_span.start, raw_span.end));
                let init_first_idx = self.nodes.len() as i32;
                self.visit_expression(init);
                if let Some(pushed) = self.nodes.get(init_first_idx as usize) {
                    if pushed.span.start_byte == raw_span.start
                        && pushed.span.end_byte == raw_span.end
                    {
                        init_idx = init_first_idx;
                    }
                }
            }
            declarations.push(VariableDeclaratorWire {
                name,
                init_idx,
                init_span,
            });
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::VariableDeclaration {
            declaration_kind,
            declarations,
        };
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let idx = self.push("Function", it.span);
        self.enter(idx);

        let name = it.id.as_ref().map(|i| i.name.to_string());
        let mut param_idxs = Vec::with_capacity(it.params.items.len());
        for param in &it.params.items {
            // Descend into the binding's pattern. For simple identifiers
            // this pushes an IdentifierReference (v0). For destructuring,
            // it descends into non-v0 nodes silently and we don't record
            // an idx (plugin still sees count via paramIdxs.length).
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                let bid = self.push("IdentifierReference", id.span);
                self.nodes[bid as usize].extras = WireExtras::Identifier {
                    name: id.name.to_string(),
                };
                param_idxs.push(bid);
            }
        }
        if let Some(body) = &it.body {
            self.visit_function_body(body);
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Function {
            name,
            param_idxs,
            is_async: it.r#async,
            generator: it.generator,
        };
        let _ = flags;
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let idx = self.push("ArrowFunctionExpression", it.span);
        self.enter(idx);

        let mut param_idxs = Vec::with_capacity(it.params.items.len());
        for param in &it.params.items {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                let bid = self.push("IdentifierReference", id.span);
                self.nodes[bid as usize].extras = WireExtras::Identifier {
                    name: id.name.to_string(),
                };
                param_idxs.push(bid);
            }
        }
        self.visit_function_body(&it.body);

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Arrow {
            param_idxs,
            is_async: it.r#async,
            expression: it.expression,
        };
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        let idx = self.push("Class", it.span);
        self.enter(idx);
        walk::walk_class(self, it);
        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Class {
            name: it.id.as_ref().map(|i| i.name.to_string()),
        };
    }

    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        let idx = self.push("ObjectExpression", it.span);
        self.enter(idx);

        let mut property_idxs = Vec::with_capacity(it.properties.len());
        for prop in &it.properties {
            // Object property entries (Property / SpreadElement) aren't
            // v0 surface kinds. Descend into the value expression so
            // any embedded v0 kinds (e.g. a nested CallExpression in a
            // shorthand) still emit; record the idx of the first
            // emission so the host can iterate properties as ranges.
            let prop_first_idx = self.nodes.len() as i32;
            match prop {
                ObjectPropertyKind::ObjectProperty(p) => {
                    self.visit_expression(&p.value);
                }
                ObjectPropertyKind::SpreadProperty(p) => {
                    self.visit_expression(&p.argument);
                }
            }
            if (self.nodes.len() as i32) > prop_first_idx {
                property_idxs.push(prop_first_idx);
            }
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Object { property_idxs };
    }

    fn visit_member_expression(&mut self, it: &MemberExpression<'a>) {
        let span = match it {
            MemberExpression::ComputedMemberExpression(m) => m.span,
            MemberExpression::StaticMemberExpression(m) => m.span,
            MemberExpression::PrivateFieldExpression(m) => m.span,
        };
        let idx = self.push("MemberExpression", span);
        self.enter(idx);

        let object_idx = self.nodes.len() as i32;
        let (object_expr, property, computed) = match it {
            MemberExpression::ComputedMemberExpression(m) => {
                // .property is an Expression — only resolves to a
                // string when statically determinable; leave None
                // otherwise.
                let prop = match &m.expression {
                    Expression::StringLiteral(s) => Some(s.value.to_string()),
                    _ => None,
                };
                (&m.object, prop, true)
            }
            MemberExpression::StaticMemberExpression(m) => {
                (&m.object, Some(m.property.name.to_string()), false)
            }
            MemberExpression::PrivateFieldExpression(m) => {
                (&m.object, Some(format!("#{}", m.field.name)), false)
            }
        };
        self.visit_expression(object_expr);
        // For computed expressions, also descend into the property
        // expression so any v0 nodes inside (e.g. obj[fn()]) emit.
        if let MemberExpression::ComputedMemberExpression(m) = it {
            if !matches!(&m.expression, Expression::StringLiteral(_)) {
                self.visit_expression(&m.expression);
            }
        }

        self.exit();
        let object_idx_final = if (object_idx as usize) < self.nodes.len() {
            object_idx
        } else {
            -1
        };
        self.nodes[idx as usize].extras = WireExtras::Member {
            object_idx: object_idx_final,
            property,
            computed,
        };
    }

    fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
        let idx = self.push("IdentifierReference", it.span);
        self.nodes[idx as usize].extras = WireExtras::Identifier {
            name: it.name.to_string(),
        };
    }

    fn visit_jsx_element(&mut self, it: &JSXElement<'a>) {
        let idx = self.push("JSXElement", it.span);
        self.enter(idx);

        let tag_name = it.opening_element.name.to_string();
        let self_closing = it.closing_element.is_none();

        let mut attribute_idxs = Vec::with_capacity(it.opening_element.attributes.len());
        for attr_item in &it.opening_element.attributes {
            let attr_idx = self.nodes.len() as i32;
            self.visit_jsx_attribute_item(attr_item);
            attribute_idxs.push(attr_idx);
        }

        for child in &it.children {
            self.visit_jsx_child(child);
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Element {
            tag_name,
            self_closing,
            attribute_idxs,
        };
    }

    fn visit_jsx_attribute(&mut self, it: &JSXAttribute<'a>) {
        let idx = self.push("JSXAttribute", it.span);
        self.enter(idx);

        let name = match &it.name {
            JSXAttributeName::Identifier(id) => id.name.to_string(),
            JSXAttributeName::NamespacedName(ns) => {
                format!("{}:{}", ns.namespace.name, ns.name.name)
            }
        };
        let (value, is_expression) = match &it.value {
            Some(JSXAttributeValue::StringLiteral(s)) => (Some(s.value.to_string()), false),
            Some(JSXAttributeValue::ExpressionContainer(_)) => (None, true),
            Some(JSXAttributeValue::Element(_) | JSXAttributeValue::Fragment(_)) | None => {
                (None, false)
            }
        };
        if let Some(v) = &it.value {
            self.visit_jsx_attribute_value(v);
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Attribute {
            name: Some(name),
            value,
            is_expression,
            is_spread: false,
        };
    }

    fn visit_jsx_spread_attribute(&mut self, it: &JSXSpreadAttribute<'a>) {
        let idx = self.push("JSXAttribute", it.span);
        self.enter(idx);
        self.visit_expression(&it.argument);
        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Attribute {
            name: None,
            value: None,
            is_expression: false,
            is_spread: true,
        };
    }

    fn visit_jsx_expression_container(&mut self, it: &JSXExpressionContainer<'a>) {
        let idx = self.push("JSXExpressionContainer", it.span);
        self.enter(idx);
        self.visit_jsx_expression(&it.expression);
        self.exit();
        self.nodes[idx as usize].extras = WireExtras::None {};
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{parse_into, Allocator, SourceFile};
    use std::path::PathBuf;

    fn build_wire(text: &str) -> AstWire {
        let file = SourceFile::new(PathBuf::from("repro.ts"), text.to_string());
        let allocator = Allocator::default();
        let parsed = parse_into(&allocator, &file);
        WireBuilder::new(text).build(&parsed.program)
    }

    fn build_wire_tsx(text: &str) -> AstWire {
        let file = SourceFile::new(PathBuf::from("repro.tsx"), text.to_string());
        let allocator = Allocator::default();
        let parsed = parse_into(&allocator, &file);
        WireBuilder::new(text).build(&parsed.program)
    }

    /// CD-191 investigation: the bead reported `JSXAttributeNode.span`
    /// shifted/truncated for a JSX element with multiple attributes on one
    /// line (e.g. `className` in `<button type="button" onClick={onClose}
    /// className={CANCEL_BTN}>` slicing to `"ame={CANCEL_BTN}>\n    "`
    /// instead of its own text). Exhaustively re-verified end-to-end
    /// against the real plugin host (single-line, multi-line-component,
    /// CRLF, and self-closing variants) and could not reproduce — every
    /// attribute's `span` slices back to exactly its own source text, both
    /// here at the wire-builder level and through the actual NDJSON
    /// pipeline `crates/cofferdam-cli/scripts/plugin-host.mjs` runs.
    /// Kept as a permanent regression guard; see CD-191 for the
    /// non-repro writeup.
    #[test]
    fn jsx_attribute_spans_are_independently_correct_on_one_line() {
        let text = "const x = <button type=\"button\" onClick={onClose} className={CANCEL_BTN}>hi</button>;\n";
        let wire = build_wire_tsx(text);
        let attrs: Vec<(&str, &str)> = wire
            .nodes
            .iter()
            .filter(|n| n.kind == "JSXAttribute")
            .map(|n| {
                let s = &n.span;
                (n.kind, &text[s.start_byte as usize..s.end_byte as usize])
            })
            .collect();
        assert_eq!(
            attrs.into_iter().map(|(_, t)| t).collect::<Vec<_>>(),
            vec![
                "type=\"button\"",
                "onClick={onClose}",
                "className={CANCEL_BTN}",
            ]
        );
    }

    fn call_extras(wire: &AstWire, idx: i32) -> (i32, &[i32]) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::Call {
                callee_idx,
                argument_idxs,
            } => (*callee_idx, argument_idxs),
            _ => panic!("node {idx} is not a CallExpression"),
        }
    }

    /// Regression test for cd-18 (github#51): a template-literal argument
    /// must not be misattributed as a truncated nested IdentifierReference.
    #[test]
    fn template_literal_argument_is_not_misattributed_as_nested_identifier() {
        let text = "await db.prepare(`SELECT * FROM users WHERE id = ${userId}`).run()\n";
        let wire = build_wire(text);

        // node[3] is the `db.prepare(...)` CallExpression (node[1] is the
        // outer `.run()` call).
        let (callee_idx, argument_idxs) = call_extras(&wire, 3);
        let callee_span = &wire.nodes[callee_idx as usize].span;
        assert_eq!(
            (callee_span.start_byte, callee_span.end_byte),
            (6, 16),
            "callee span (`db.prepare`) must stay correct"
        );

        // The template-literal argument has no v0 representation of its
        // own (TemplateLiteral is out of the frozen v0 NodeKind union —
        // see design/sdk-ast-surface.md), so it must be dropped from
        // argumentIdxs entirely rather than misattributed to whichever
        // v0 descendant happens to sit inside it.
        assert_eq!(
            argument_idxs,
            &[] as &[i32],
            "template-literal argument must be omitted, not misattributed"
        );

        // The nested `userId` identifier must still be discoverable in
        // the flat array (findAll/walk still reach it) — the fix must
        // not stop traversal into non-v0 argument kinds, only stop the
        // argument-idx misattribution.
        let userid_node = wire
            .nodes
            .iter()
            .find(|n| n.kind == "IdentifierReference" && n.span.start_byte == 51)
            .expect("nested `userId` identifier reference must still be emitted");
        assert_eq!(userid_node.span.end_byte, 57);
    }

    /// Non-template argument kinds (identifier, object expression) must
    /// keep reporting their own full span in argumentIdxs, unaffected by
    /// the cd-18 fix. A string-literal argument has no v0 representation
    /// (frozen out, same as TemplateLiteral) and is correctly omitted —
    /// this was already true before the fix and must stay true.
    #[test]
    fn non_template_arguments_are_unaffected() {
        let text = r#"send({ a: 1 }, target, "literal");"#;
        let wire = build_wire(text);

        // node[0] is Program, node[1] is the CallExpression.
        let (_, argument_idxs) = call_extras(&wire, 1);
        assert_eq!(
            argument_idxs.len(),
            2,
            "object + identifier args recorded; string literal omitted (frozen out of v0)"
        );

        let object_node = &wire.nodes[argument_idxs[0] as usize];
        assert_eq!(object_node.kind, "ObjectExpression");
        assert_eq!(object_node.span.start_byte, 5);
        assert_eq!(object_node.span.end_byte, 13);

        let ident_node = &wire.nodes[argument_idxs[1] as usize];
        assert_eq!(ident_node.kind, "IdentifierReference");
        assert_eq!(ident_node.span.start_byte, 15);
        assert_eq!(ident_node.span.end_byte, 21);
    }

    fn element_extras(wire: &AstWire, idx: i32) -> (&str, bool, &[i32]) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::Element {
                tag_name,
                self_closing,
                attribute_idxs,
            } => (tag_name, *self_closing, attribute_idxs),
            _ => panic!("node {idx} is not a JSXElement"),
        }
    }

    fn attribute_extras(wire: &AstWire, idx: i32) -> (Option<&str>, Option<&str>, bool, bool) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::Attribute {
                name,
                value,
                is_expression,
                is_spread,
            } => (
                name.as_deref(),
                value.as_deref(),
                *is_expression,
                *is_spread,
            ),
            _ => panic!("node {idx} is not a JSXAttribute"),
        }
    }

    #[test]
    fn jsx_element_with_static_and_expression_attributes() {
        let wire = build_wire_tsx(r#"const x = <img src="cat.png" alt={caption} />;"#);

        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "JSXElement")
            .expect("JSXElement must be emitted") as i32;
        let (tag_name, self_closing, attribute_idxs) = element_extras(&wire, el_idx);
        assert_eq!(tag_name, "img");
        assert!(self_closing, "`<img ... />` has no closing element");
        assert_eq!(attribute_idxs.len(), 2, "src + alt attributes");

        let (src_name, src_value, src_is_expr, src_is_spread) =
            attribute_extras(&wire, attribute_idxs[0]);
        assert_eq!(src_name, Some("src"));
        assert_eq!(src_value, Some("cat.png"));
        assert!(!src_is_expr);
        assert!(!src_is_spread);

        let (alt_name, alt_value, alt_is_expr, alt_is_spread) =
            attribute_extras(&wire, attribute_idxs[1]);
        assert_eq!(alt_name, Some("alt"));
        assert_eq!(alt_value, None, "expression container has no literal value");
        assert!(alt_is_expr);
        assert!(!alt_is_spread);

        // The expression container wraps an IdentifierReference (`caption`) —
        // it must still be discoverable via the flat array.
        assert!(
            wire.nodes
                .iter()
                .any(|n| n.kind == "JSXExpressionContainer"),
            "JSXExpressionContainer must be emitted for `alt={{caption}}`"
        );
        assert!(
            wire.nodes.iter().any(|n| n.kind == "IdentifierReference"
                && matches!(&n.extras, WireExtras::Identifier { name } if name == "caption")),
            "the identifier inside the expression container must still be discoverable"
        );
    }

    #[test]
    fn jsx_member_expression_tag_name() {
        let wire = build_wire_tsx(r#"const x = <Image.Src url="a.png" />;"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "JSXElement")
            .expect("JSXElement must be emitted") as i32;
        let (tag_name, _, _) = element_extras(&wire, el_idx);
        assert_eq!(
            tag_name, "Image.Src",
            "member-expression tag names flatten via Display"
        );
    }

    #[test]
    fn jsx_spread_attribute_is_marked_not_missing() {
        let wire = build_wire_tsx(r#"const x = <Foo {...rest} />;"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "JSXElement")
            .expect("JSXElement must be emitted") as i32;
        let (_, _, attribute_idxs) = element_extras(&wire, el_idx);
        assert_eq!(
            attribute_idxs.len(),
            1,
            "the spread counts as one attribute item"
        );

        let (name, value, is_expression, is_spread) = attribute_extras(&wire, attribute_idxs[0]);
        assert_eq!(name, None, "spread attributes have no name");
        assert_eq!(value, None);
        assert!(!is_expression);
        assert!(
            is_spread,
            "spread must be flagged so it isn't mistaken for a missing attribute"
        );
    }

    fn var_decl_extras(wire: &AstWire, idx: i32) -> (&str, &[VariableDeclaratorWire]) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::VariableDeclaration {
                declaration_kind,
                declarations,
            } => (declaration_kind, declarations),
            _ => panic!("node {idx} is not a VariableDeclaration"),
        }
    }

    /// CD-78: `findAll("VariableDeclaration")` must surface `const`, `let`,
    /// and destructuring declarations with correct spans, the declarator
    /// name(s), the `const`/`let`/`var` kind, and the init node index.
    #[test]
    fn variable_declaration_const_let_and_destructuring() {
        let text = "const a = foo();\nlet b = 2;\nconst { x, y } = obj;\n";
        let wire = build_wire(text);

        let vd_idxs: Vec<i32> = wire
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == "VariableDeclaration")
            .map(|(i, _)| i as i32)
            .collect();
        assert_eq!(vd_idxs.len(), 3, "one node per declaration statement");

        // `const a = foo();`
        let (kind0, decls0) = var_decl_extras(&wire, vd_idxs[0]);
        assert_eq!(kind0, "const");
        assert_eq!(decls0.len(), 1);
        assert_eq!(decls0[0].name.as_deref(), Some("a"));
        let span0 = &wire.nodes[vd_idxs[0] as usize].span;
        assert_eq!(
            &text[span0.start_byte as usize..span0.end_byte as usize],
            "const a = foo();"
        );
        // init is a CallExpression (a v0 kind) → linked.
        let init0 = &wire.nodes[decls0[0].init_idx as usize];
        assert_eq!(init0.kind, "CallExpression");

        // `let b = 2;` — numeric literal init is not a v0 kind → init_idx == -1,
        // but init_span (CD-190) still covers just the literal.
        let (kind1, decls1) = var_decl_extras(&wire, vd_idxs[1]);
        assert_eq!(kind1, "let");
        assert_eq!(decls1.len(), 1);
        assert_eq!(decls1[0].name.as_deref(), Some("b"));
        assert_eq!(
            decls1[0].init_idx, -1,
            "numeric literal init has no v0 node"
        );
        let init_span1 = decls1[0]
            .init_span
            .expect("init_span set for a present initializer");
        assert_eq!(
            &text[init_span1.start_byte as usize..init_span1.end_byte as usize],
            "2"
        );

        // `const { x, y } = obj;` — one declarator, object-pattern binding →
        // name is None (destructuring left to a follow-up per CD-78).
        let (kind2, decls2) = var_decl_extras(&wire, vd_idxs[2]);
        assert_eq!(kind2, "const");
        assert_eq!(decls2.len(), 1, "one declarator with an object-pattern id");
        assert_eq!(
            decls2[0].name, None,
            "destructuring declarator has no simple name"
        );
        let span2 = &wire.nodes[vd_idxs[2] as usize].span;
        assert_eq!(
            &text[span2.start_byte as usize..span2.end_byte as usize],
            "const { x, y } = obj;"
        );
    }

    /// CD-190: `init_idx` stays `-1` for a string-literal or
    /// binary-expression initializer (neither is a v0 wire node kind), but
    /// `init_span` must still isolate just the initializer's own source
    /// text rather than forcing callers to fall back to the whole
    /// `VariableDeclaration`'s span.
    #[test]
    fn variable_declaration_init_span_covers_literal_and_binary_initializers() {
        let text = "const SEND_BTN = 'bg-brass text-white';\nconst GREETING = 'a' + 'b';\n";
        let wire = build_wire(text);

        let vd_idxs: Vec<i32> = wire
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == "VariableDeclaration")
            .map(|(i, _)| i as i32)
            .collect();
        assert_eq!(vd_idxs.len(), 2);

        let (_, decls0) = var_decl_extras(&wire, vd_idxs[0]);
        assert_eq!(decls0[0].init_idx, -1, "string literal init has no v0 node");
        let span0 = decls0[0]
            .init_span
            .expect("init_span set for a present initializer");
        assert_eq!(
            &text[span0.start_byte as usize..span0.end_byte as usize],
            "'bg-brass text-white'"
        );

        let (_, decls1) = var_decl_extras(&wire, vd_idxs[1]);
        assert_eq!(
            decls1[0].init_idx, -1,
            "binary expression init has no v0 node"
        );
        let span1 = decls1[0]
            .init_span
            .expect("init_span set for a present initializer");
        assert_eq!(
            &text[span1.start_byte as usize..span1.end_byte as usize],
            "'a' + 'b'"
        );
    }

    #[test]
    fn jsx_namespaced_attribute_name() {
        let wire = build_wire_tsx(r#"const x = <svg xml:lang="en" />;"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "JSXElement")
            .expect("JSXElement must be emitted") as i32;
        let (_, _, attribute_idxs) = element_extras(&wire, el_idx);
        assert_eq!(attribute_idxs.len(), 1);

        let (name, value, is_expression, is_spread) = attribute_extras(&wire, attribute_idxs[0]);
        assert_eq!(
            name,
            Some("xml:lang"),
            "namespaced attribute names flatten to `namespace:name`"
        );
        assert_eq!(value, Some("en"));
        assert!(!is_expression);
        assert!(!is_spread);
    }
}
