use std::rc::Rc;

use zeen_ast::expressions::{BinaryOp, Literal, UnaryOp};
use zeen_hir::{
    HirDecl, HirDeclKind, HirExpr, HirExprKind, HirFn, HirMacroKind, HirModule, HirStmt,
    HirStmtKind, HirTypeExpr, HirTypeKind,
};
use zeen_resolve::DefId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralHint {
    Int,
    Float,
    Char,
    Bool,
}

#[derive(Debug, Clone)]
pub enum HintSource {
    Literal(LiteralHint),
    Annotation(Rc<HirTypeExpr>),
    ParamType(DefId, usize),
}

impl HintSource {
    pub fn priority(&self) -> u8 {
        match self {
            HintSource::Annotation(_) => 0,
            HintSource::ParamType(_, _) => 1,
            HintSource::Literal(_) => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hint {
    pub target: DefId,
    pub source: HintSource,
}

pub fn collect(module: &HirModule) -> Vec<Hint> {
    let mut collector = Collector {
        hints: Vec::new(),
        return_types: Vec::new(),
    };

    for decl in &module.decls {
        match &decl.kind {
            HirDeclKind::Fn(f) => collector.fn_body(f),

            HirDeclKind::Struct(s) => {
                for method in &s.methods {
                    collector.decl(method);
                }
            }

            HirDeclKind::Interface(i) => {
                for method in &i.methods {
                    collector.decl(method);
                }
            }

            HirDeclKind::Implement(imp) => {
                for method in &imp.methods {
                    collector.decl(method);
                }
            }

            HirDeclKind::GlobalVar { value, .. } => collector.expr(value),

            _ => {}
        }
    }

    collector.hints
}

struct Collector {
    hints: Vec<Hint>,
    return_types: Vec<Option<Rc<HirTypeExpr>>>,
}

impl Collector {
    fn fn_body(&mut self, f: &HirFn) {
        self.return_types.push(f.return_type.clone());

        if let Some(body) = &f.body {
            self.stmt(body);
        }

        self.return_types.pop();
    }

    fn decl(&mut self, decl: &HirDecl) {
        if let HirDeclKind::Fn(f) = &decl.kind {
            self.fn_body(f);
        }
    }

    fn stmt(&mut self, stmt: &HirStmt) {
        match &stmt.kind {
            HirStmtKind::Let {
                explicit_type,
                value,
                ..
            } => {
                self.annotated_value(explicit_type.as_ref(), value.as_deref());

                if let Some(value) = value {
                    self.expr(value);
                }
            }

            HirStmtKind::Assign { object, value }
            | HirStmtKind::CompoundAssign { object, value, .. } => {
                self.assigned_value(object, value);

                self.expr(object);
                self.expr(value);
            }

            HirStmtKind::Return { value } => {
                self.returned_value(value.as_deref());

                if let Some(value) = value {
                    self.expr(value);
                }
            }

            HirStmtKind::While {
                condition,
                update,
                block,
                else_block,
                ..
            } => {
                self.expr(condition);

                if let Some(update) = update {
                    self.stmt(update);
                }

                self.stmt(block);

                if let Some(else_block) = else_block {
                    self.stmt(else_block);
                }
            }

            HirStmtKind::For {
                iterator,
                block,
                else_block,
                ..
            } => {
                self.expr(iterator);
                self.stmt(block);

                if let Some(else_block) = else_block {
                    self.stmt(else_block);
                }
            }

            HirStmtKind::Expr(expr) => self.expr(expr),

            HirStmtKind::FnDecl(decl) => self.decl(decl),

            HirStmtKind::Break { .. } | HirStmtKind::Continue { .. } | HirStmtKind::Error => {}
        }
    }

    fn expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::Binary { lhs, rhs, op } => {
                self.binary(lhs, rhs, *op);
                self.expr(lhs);
                self.expr(rhs);
            }

            HirExprKind::Call { callee, args, .. } => {
                self.call(callee, args);

                self.expr(callee);

                for arg in args {
                    self.expr(arg);
                }
            }

            HirExprKind::MacroCall { kind, args } => {
                self.macro_call(kind.0, args);

                for arg in args {
                    self.expr(arg);
                }
            }

            HirExprKind::Closure { def, .. } => self.fn_body(def),

            HirExprKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.expr(condition);
                self.stmt(then_block);

                if let Some(else_block) = else_block {
                    self.stmt(else_block);
                }
            }

            HirExprKind::Switch { object, arms } => {
                self.expr(object);

                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }

                    self.expr(&arm.body);
                }
            }

            HirExprKind::FieldAccess { object, .. } => self.expr(object),

            HirExprKind::SliceAccess { object, index } => {
                self.expr(object);
                self.expr(index);
            }

            HirExprKind::StructInit { fields, .. } => {
                for field in fields {
                    self.expr(&field.value);
                }
            }

            HirExprKind::AnonymousStructInit { fields } => {
                for field in fields {
                    self.expr(&field.value);
                }
            }

            HirExprKind::ArrayInit { elements } => {
                for element in elements {
                    self.expr(element);
                }
            }

            HirExprKind::ArrayRepeatInit { element, len } => {
                self.expr(element);
                self.expr(len);
            }

            HirExprKind::Block { stmts, trailing } => {
                for stmt in stmts {
                    self.stmt(stmt);
                }

                if let Some(trailing) = trailing {
                    self.expr(trailing);
                }
            }

            HirExprKind::Unary { expr, .. } => self.expr(expr),

            HirExprKind::Try { expr } => self.expr(expr),

            HirExprKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.expr(start);
                }

                if let Some(end) = end {
                    self.expr(end);
                }
            }

            HirExprKind::Literal(_)
            | HirExprKind::VarRef { .. }
            | HirExprKind::GenericParamRef(_)
            | HirExprKind::SelfValue(_)
            | HirExprKind::Type(_)
            | HirExprKind::Error => {}
        }
    }

    fn annotated_value(&mut self, annotation: Option<&Rc<HirTypeExpr>>, value: Option<&HirExpr>) {
        let (Some(annotation), Some(value)) = (annotation, value) else {
            return;
        };

        self.expected(annotation, value);
    }

    fn assigned_value(&mut self, object: &HirExpr, value: &HirExpr) {
        let (Some(target), Some(literal)) = (var_ref(object), literal_hint(value)) else {
            return;
        };

        self.hints.push(Hint {
            target,
            source: HintSource::Literal(literal),
        });
    }

    fn returned_value(&mut self, value: Option<&HirExpr>) {
        let Some(value) = value else {
            return;
        };

        let Some(Some(annotation)) = self.return_types.last().cloned() else {
            return;
        };

        self.expected(&annotation, value);
    }

    fn expected(&mut self, ty: &Rc<HirTypeExpr>, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::VarRef { def, .. } => self.annotation_hint(*def, ty),

            HirExprKind::Binary { lhs, rhs, op } if propagates_expected(*op) => {
                self.expected(ty, lhs);
                self.expected(ty, rhs);
            }

            HirExprKind::Unary {
                expr,
                op: UnaryOp::Neg | UnaryOp::Not | UnaryOp::BitNot,
            } => {
                self.expected(ty, expr);
            }

            HirExprKind::ArrayInit { elements } => {
                if let Some(element) = expected_element(ty) {
                    for item in elements {
                        self.expected(element, item);
                    }
                }
            }

            HirExprKind::ArrayRepeatInit { element, .. } => {
                if let Some(item_ty) = expected_element(ty) {
                    self.expected(item_ty, element);
                }
            }

            HirExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                self.expected_stmt(ty, then_block);

                if let Some(else_block) = else_block {
                    self.expected_stmt(ty, else_block);
                }
            }

            HirExprKind::Switch { arms, .. } => {
                for arm in arms {
                    self.expected(ty, &arm.body);
                }
            }

            HirExprKind::Block {
                trailing: Some(trailing),
                ..
            } => self.expected(ty, trailing),

            _ => {}
        }
    }

    fn expected_stmt(&mut self, ty: &Rc<HirTypeExpr>, stmt: &HirStmt) {
        if let HirStmtKind::Expr(expr) = &stmt.kind {
            self.expected(ty, expr);
        }
    }

    fn annotation_hint(&mut self, target: DefId, ty: &Rc<HirTypeExpr>) {
        self.hints.push(Hint {
            target,
            source: HintSource::Annotation(ty.clone()),
        });
    }

    fn binary(&mut self, lhs: &HirExpr, rhs: &HirExpr, op: BinaryOp) {
        if !hints_operand(op) {
            return;
        }

        if let Some(literal) = literal_hint(lhs)
            && let Some(target) = var_ref(rhs)
        {
            self.hints.push(Hint {
                target,
                source: HintSource::Literal(literal),
            });
        }

        if let Some(literal) = literal_hint(rhs)
            && let Some(target) = var_ref(lhs)
        {
            self.hints.push(Hint {
                target,
                source: HintSource::Literal(literal),
            });
        }
    }

    fn call(&mut self, callee: &HirExpr, args: &[Rc<HirExpr>]) {
        let Some(callee_def) = var_ref(callee) else {
            return;
        };

        for (index, arg) in args.iter().enumerate() {
            let Some(target) = var_ref(arg) else {
                continue;
            };

            self.hints.push(Hint {
                target,
                source: HintSource::ParamType(callee_def, index),
            });
        }
    }

    fn macro_call(&mut self, kind: HirMacroKind, args: &[Rc<HirExpr>]) {
        if kind != HirMacroKind::As {
            return;
        }

        let [ty, value, ..] = args else {
            return;
        };

        let HirExprKind::Type(ty) = &ty.kind else {
            return;
        };

        self.expected(ty, value);
    }
}

fn expected_element(ty: &Rc<HirTypeExpr>) -> Option<&Rc<HirTypeExpr>> {
    match &ty.kind {
        HirTypeKind::Array { element, .. } => Some(element),
        HirTypeKind::ManyPointer(element) => Some(element),
        _ => None,
    }
}

pub fn propagates_expected(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
    )
}

fn hints_operand(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge
    )
}

fn literal_hint(expr: &HirExpr) -> Option<LiteralHint> {
    match &expr.kind {
        HirExprKind::Literal(Literal::Int(_)) => Some(LiteralHint::Int),
        HirExprKind::Literal(Literal::Float(_)) => Some(LiteralHint::Float),
        HirExprKind::Literal(Literal::Char(_)) => Some(LiteralHint::Char),
        HirExprKind::Literal(Literal::Bool(_)) => Some(LiteralHint::Bool),
        _ => None,
    }
}

fn var_ref(expr: &HirExpr) -> Option<DefId> {
    match &expr.kind {
        HirExprKind::VarRef { def, .. } => Some(*def),
        _ => None,
    }
}
