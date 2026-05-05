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
use cofferdam_ts::oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, CallExpression, Class, Expression, Function,
    ImportDeclaration, ImportDeclarationSpecifier, MemberExpression, ObjectExpression,
    ObjectPropertyKind, Program,
};
use cofferdam_ts::oxc_ast_visit::{walk, Visit};
use cofferdam_ts::oxc_syntax::scope::ScopeFlags;
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
}

#[derive(Serialize)]
pub struct ImportSpecifierWire {
    #[serde(rename = "localName")]
    pub local_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported: Option<String>,
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
    fn push(&mut self, kind: &'static str, span: cofferdam_ts::oxc_span::Span) -> i32 {
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
            let arg_first_idx = self.nodes.len() as i32;
            self.visit_argument(arg);
            // Only record an argument idx if the visit actually pushed
            // a v0 node. `nodes.len()` advanced means yes.
            if (self.nodes.len() as i32) > arg_first_idx {
                argument_idxs.push(arg_first_idx);
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

    fn visit_identifier_reference(
        &mut self,
        it: &cofferdam_ts::oxc_ast::ast::IdentifierReference<'a>,
    ) {
        let idx = self.push("IdentifierReference", it.span);
        self.nodes[idx as usize].extras = WireExtras::Identifier {
            name: it.name.to_string(),
        };
    }
}
