//! From the XQuery syntax tree to `xee`'s intermediate representation (`xee-ir`), which `xee`'s own compiler turns into bytecode.
//!
//! The XPath part follows `xee-xpath-compiler`'s converter (MIT, Paligo; `IrConverter` in `ast_ir.rs`): each expression becomes a
//! list of bindings whose last one holds its value. XQuery adds:
//! - **The prolog.** Declared functions and global variables live in an *environment*, an array `[functions, $g1, $g2, …]` that
//!   every declared function takes as a hidden first argument. A call `local:f($a)` is `$env(1)(k)($env, $a)`, so functions can
//!   call themselves and each other; a global variable is `$env(i + 1)`. The query body and the initializers see it as a
//!   variable.
//! - **FLWOR.** `for`, `let`, `where` nest directly (a map over the binding sequence, a `let`, an `if`). `order by`, `group by`
//!   and `count` need the whole tuple stream: the clauses before them return one array per tuple (`[$v1, …, $vn, keys…]`), the
//!   array sequence is sorted or grouped by functions of the prelude ([`crate::prelude`], XQuery itself), and the clauses after
//!   them run over the arrays with the variables bound again. Windows are found by prelude functions too, from the start and
//!   end conditions compiled as functions.
//! - **Constructors.** The node-building instructions `xee` has for XSLT (`XmlElement`, `XmlAttribute`, `XmlAppend`, …).
//! - **`try`/`catch`.** A call of `fn:xmlcraft-try` (a function of the vendored interpreter) with the body and a handler.

use std::collections::HashMap;

use xee_interpreter::context::StaticContext;
use xee_interpreter::error::SpannedError;
use xee_ir::{Bindings, Variables, ir};
use xee_schema_type::Xs;
use xee_xpath_ast::ast as xast;
use xee_xpath_ast::span::Spanned as XSpanned;
use xot::xmlname::NameStrInfo;

use crate::ast::*;
use crate::parse::{ERR_NS, FN_NS, PREDECLARED};

pub(crate) const INTERNAL_NS: &str = "http://xmlcraft.org/ns/xquery-internal";

type XSpan = xast::Span;

/// A static error found while compiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub code: String,
    pub message: String,
    pub span: Option<Span>,
}

type C<T> = Result<T, CompileError>;

fn from_xee(e: SpannedError) -> CompileError {
    let span = e.span.map(|s| s.range()).filter(|r| r.end > 0);
    let message = e.error.message().to_owned();
    CompileError { code: e.error.code(), message, span }
}

fn cerr(code: &str, message: impl Into<String>, span: &Span) -> CompileError {
    CompileError { code: code.to_owned(), message: message.into(), span: Some(span.clone()) }
}

fn internal(local: &str) -> Name {
    Name::new(local.to_owned(), INTERNAL_NS.to_owned(), String::new())
}

fn err_name(local: &str) -> Name {
    Name::new(local.to_owned(), ERR_NS.to_owned(), "err".to_owned())
}

fn item_type(item_type: xast::ItemType, occurrence: xast::Occurrence) -> SequenceType {
    xast::SequenceType::Item(xast::Item { item_type, occurrence })
}

/// What the compiler is told besides the module.
pub(crate) struct Setup<'a> {
    /// External variables the caller gives values for, in the order of the static context's variable names.
    pub supplied: &'a [Name],
    /// Whether the caller gives a context item.
    pub has_context: bool,
    /// Prefixes the caller declared.
    pub namespaces: &'a [(String, String)],
}

pub(crate) struct Compiler<'a> {
    v: Variables,
    sc: &'a StaticContext,
    /// Variables bound by the expression around the place being compiled (innermost last).
    locals: Vec<Name>,
    /// Global variables, by position (`$env(i + 2)`).
    globals: Vec<Name>,
    /// Global variables that are external without a value and without a default: referring to one is XPDY0002.
    missing: Vec<Name>,
    /// Declared functions (the prelude's too): name and arity, by position (`$env(1)(i + 1)`).
    functions: Vec<(Name, usize)>,
    env: Name,
    boundary_preserve: bool,
    empty_greatest: bool,
    /// While the prelude is compiled, its spans are not places in the query.
    no_spans: bool,
    /// The statically known prefixes (the predeclared ones, the caller's and the prolog's), and those of the direct
    /// constructors around the place being compiled.
    prefixes: HashMap<String, String>,
    ns_stack: Vec<Vec<(String, String)>>,
    default_element: String,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(sc: &'a StaticContext) -> Self {
        Compiler {
            v: Variables::new(),
            sc,
            locals: Vec::new(),
            globals: Vec::new(),
            missing: Vec::new(),
            functions: Vec::new(),
            env: internal("env"),
            boundary_preserve: false,
            empty_greatest: false,
            no_spans: false,
            prefixes: HashMap::new(),
            ns_stack: Vec::new(),
            default_element: String::new(),
        }
    }

    fn sp(&self, s: &Span) -> XSpan {
        if self.no_spans { (0..0).into() } else { (s.start..s.end).into() }
    }

    // ---- small builders ----

    fn bind(&mut self, expr: ir::Expr, span: XSpan) -> Bindings {
        Bindings::new(self.v.new_binding(expr, span))
    }

    /// One binding holding the value of `b` (so that long binding lists do not nest too deeply).
    fn seal(&mut self, b: Bindings) -> Bindings {
        let e = b.expr();
        Bindings::new(self.v.new_binding(e.value, e.span))
    }

    fn konst(&self, c: ir::Const, span: XSpan) -> ir::AtomS {
        XSpanned::new(ir::Atom::Const(c), span)
    }

    fn string(&self, s: &str, span: XSpan) -> ir::AtomS {
        self.konst(ir::Const::String(s.to_owned()), span)
    }

    fn int(&self, i: usize, span: XSpan) -> ir::AtomS {
        self.konst(ir::Const::Integer(ibig::IBig::from(i)), span)
    }

    fn var_atom(&self, name: &ir::Name, span: XSpan) -> ir::AtomS {
        XSpanned::new(ir::Atom::Variable(name.clone()), span)
    }

    fn atom_b(&mut self, atom: ir::AtomS) -> Bindings {
        let span = atom.span;
        self.bind(ir::Expr::Atom(atom), span)
    }

    fn empty(&mut self, span: XSpan) -> Bindings {
        let a = self.konst(ir::Const::EmptySequence, span);
        self.atom_b(a)
    }

    fn empty_expr(&self, span: XSpan) -> ir::ExprS {
        XSpanned::new(ir::Expr::Atom(self.konst(ir::Const::EmptySequence, span)), span)
    }

    fn static_ref(&mut self, local: &str, ns: &str, arity: usize, span: XSpan) -> C<Bindings> {
        let name = Name::new(local.to_owned(), ns.to_owned(), String::new());
        let arity8 = u8::try_from(arity).map_err(|_| CompileError { code: "XPST0017".into(), message: "too many arguments".into(), span: None })?;
        let id = self.sc.function_id_by_name(&name, arity8).ok_or_else(|| CompileError {
            code: "XPST0017".into(),
            message: format!("the engine has no function {local}#{arity}"),
            span: None,
        })?;
        let atom = ir::Atom::Const(ir::Const::StaticFunctionReference(id, self.v.current_context_names()));
        Ok(self.bind(ir::Expr::Atom(XSpanned::new(atom, span)), span))
    }

    /// Call a library function with atoms as arguments.
    fn call_fn(&mut self, local: &str, ns: &str, args: Vec<ir::AtomS>, span: XSpan) -> C<Bindings> {
        let mut f = self.static_ref(local, ns, args.len(), span)?;
        let fa = f.atom();
        let call = self.v.new_binding(ir::Expr::FunctionCall(ir::FunctionCall { atom: fa, args }), span);
        Ok(f.bind(call))
    }

    fn call_atom(&mut self, function: ir::AtomS, args: Vec<ir::AtomS>, span: XSpan) -> Bindings {
        self.bind(ir::Expr::FunctionCall(ir::FunctionCall { atom: function, args }), span)
    }

    /// `left op right` on two bindings.
    fn binary(&mut self, mut left: Bindings, op: xast::BinaryOperator, mut right: Bindings, span: XSpan) -> Bindings {
        let la = left.atom();
        let ra = right.atom();
        let b = self.v.new_binding(ir::Expr::Binary(ir::Binary { left: la, op, right: ra }), span);
        left.concat(right).bind(b)
    }

    /// The comma sequence of atoms.
    fn sequence_of(&mut self, atoms: Vec<ir::AtomS>, span: XSpan) -> Bindings {
        let mut iter = atoms.into_iter();
        let Some(first) = iter.next() else { return self.empty(span) };
        let mut b = self.atom_b(first);
        for a in iter {
            let next = self.atom_b(a);
            b = self.binary(b, xast::BinaryOperator::Comma, next, span);
        }
        b
    }

    /// `fn:error(QName(err, code), message)`.
    fn raise(&mut self, code: &str, message: &str, span: XSpan) -> C<Bindings> {
        let ns = self.string(ERR_NS, span);
        let local = self.string(&format!("err:{code}"), span);
        let mut q = self.call_fn("QName", FN_NS, vec![ns, local], span)?;
        let qa = q.atom();
        let m = self.string(message, span);
        let call = self.call_fn("error", FN_NS, vec![qa, m], span)?;
        Ok(q.concat(call))
    }

    fn env_ref(&mut self, span: XSpan) -> C<Bindings> {
        self.v.var_ref(&self.env, span).map_err(from_xee)
    }

    /// `$env(index)`
    fn env_member(&mut self, index: usize, span: XSpan) -> C<Bindings> {
        let mut env = self.env_ref(span)?;
        let ea = env.atom();
        let i = self.int(index, span);
        let call = self.call_atom(ea, vec![i], span);
        Ok(env.concat(call))
    }

    /// Call a declared function (or one of the prelude) by its position, with atoms as arguments.
    fn call_user(&mut self, index: usize, args: Vec<ir::AtomS>, span: XSpan) -> C<Bindings> {
        let mut fns = self.env_member(1, span)?;
        let fa = fns.atom();
        let k = self.int(index.saturating_add(1), span);
        let mut f = self.call_atom(fa, vec![k], span);
        let f_atom = f.atom();
        let mut env = self.env_ref(span)?;
        let env_atom = env.atom();
        let mut all = vec![env_atom];
        all.extend(args);
        let call = self.call_atom(f_atom, all, span);
        Ok(fns.concat(f).concat(env).concat(call))
    }

    fn prelude_call(&mut self, local: &str, args: Vec<ir::AtomS>, span: XSpan) -> C<Bindings> {
        let name = internal(local);
        let arity = args.len();
        let Some(index) = self.functions.iter().position(|(n, a)| *n == name && *a == arity) else {
            return Err(CompileError { code: "XPST0017".into(), message: format!("the prelude has no {local}#{arity}"), span: None });
        };
        self.call_user(index, args, span)
    }

    fn type_check(&mut self, var: &VarBinding, body: &mut dyn FnMut(&mut Self) -> C<Bindings>) -> C<Bindings> {
        let Some(t) = &var.type_ else { return body(self) };
        let span = self.sp(&var.span);
        let mut vr = self.v.var_ref(&var.name, span).map_err(from_xee)?;
        let va = vr.atom();
        let check = self.v.new_binding(ir::Expr::InstanceOf(ir::InstanceOf { atom: va, sequence_type: t.clone() }), span);
        let mut cond = vr.bind(check);
        let ca = cond.atom();
        let then = body(self)?;
        let message = format!("the value of ${} does not match its declared type", var.name.local_name());
        let else_ = self.raise("XPTY0004", &message, span)?;
        let e = self.v.new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(then.expr()), else_: Box::new(else_.expr()) }), span);
        Ok(cond.bind(e))
    }

    // ---- the module ----

    /// Compile a main module (with the prelude's functions) to the outer function definition `xee` runs.
    pub(crate) fn module(mut self, module: &Module, prelude: &Module, setup: &Setup<'_>) -> C<ir::ExprS> {
        for (p, u) in PREDECLARED {
            self.prefixes.insert((*p).to_owned(), (*u).to_owned());
        }
        for (p, u) in setup.namespaces {
            self.prefixes.insert(p.clone(), u.clone());
        }
        let mut context_decl: Option<(&Option<SequenceType>, bool, &Option<ExprS>, Span)> = None;
        let mut function_decls: Vec<(&FunctionDecl, bool)> = Vec::new();
        let mut variable_decls: Vec<&Spanned<Decl>> = Vec::new();
        for decl in &module.prolog {
            match &decl.value {
                Decl::Namespace { prefix, uri } => {
                    if uri.is_empty() {
                        self.prefixes.remove(prefix);
                    } else {
                        self.prefixes.insert(prefix.clone(), uri.clone());
                    }
                }
                Decl::DefaultElementNamespace(uri) => self.default_element = uri.clone(),
                Decl::BoundarySpace(preserve) => self.boundary_preserve = *preserve,
                Decl::EmptyOrder(greatest) => self.empty_greatest = *greatest,
                Decl::DefaultCollation(uri) => {
                    if uri != "http://www.w3.org/2005/xpath-functions/collation/codepoint" {
                        return Err(cerr(
                            "XQST0038",
                            format!("the collation '{uri}' is not supported (only the codepoint collation is)"),
                            &decl.span,
                        ));
                    }
                }
                Decl::SchemaImport { .. } => {
                    return Err(cerr("XQST0009", "schema import is not supported (the engine is not schema-aware)", &decl.span));
                }
                Decl::ModuleImport { uri, .. } => {
                    return Err(cerr("XQST0059", format!("the module '{uri}' cannot be loaded: importing modules is not supported yet"), &decl.span));
                }
                Decl::ContextItem { type_, external, value } => context_decl = Some((type_, *external, value, decl.span.clone())),
                Decl::Function(f) => function_decls.push((f, false)),
                Decl::Variable { .. } => variable_decls.push(decl),
                _ => {}
            }
        }
        for decl in &prelude.prolog {
            if let Decl::Function(f) = &decl.value {
                function_decls.push((f, true));
            }
        }
        for (f, _) in &function_decls {
            let key = (f.name.clone(), f.params.len());
            if self.functions.contains(&key) {
                return Err(CompileError {
                    code: "XQST0034".into(),
                    message: format!("the function {}#{} is declared twice", f.name.local_name(), f.params.len()),
                    span: f.body.as_ref().map(|b| b.span.clone()),
                });
            }
            self.functions.push(key);
        }
        for decl in &variable_decls {
            if let Decl::Variable { name, external, value, .. } = &decl.value {
                if self.globals.contains(name) {
                    return Err(cerr("XQST0049", format!("the variable ${} is declared twice", name.local_name()), &decl.span));
                }
                self.globals.push(name.clone());
                if *external && value.is_none() && !setup.supplied.contains(name) {
                    self.missing.push(name.clone());
                }
            }
        }

        let main_context = self.v.push_context();
        let supplied_ir: Vec<ir::Name> = setup.supplied.iter().map(|n| self.v.new_var_name(n)).collect();
        let fns_ir = self.v.new_var_name(&internal("fns"));
        let env_ir = self.v.new_var_name(&self.env.clone());
        let span0: XSpan = (0..0).into();

        // the functions
        let mut fn_atoms = Vec::new();
        let mut fns_b = Bindings::empty();
        for (f, is_prelude) in &function_decls {
            self.no_spans = *is_prelude;
            let Some(body) = &f.body else {
                return Err(CompileError {
                    code: "XPST0017".into(),
                    message: format!("the external function {} is not available", f.name.local_name()),
                    span: None,
                });
            };
            let def = self.function_definition(&f.params, &f.return_type, body, true)?;
            let mut b = self.bind(ir::Expr::FunctionDefinition(def), span0);
            fn_atoms.push(b.atom());
            fns_b = fns_b.concat(b);
        }
        self.no_spans = false;
        let fns_array = self.v.new_binding(ir::Expr::ArrayConstructor(ir::ArrayConstructor::Square(fn_atoms)), span0);
        let fns_value = fns_b.bind(fns_array).expr();

        // the context item of the prolog's declaration, when it sets one
        let mut context_value: Option<(Bindings, ir::ContextNames)> = None;
        if let Some((type_, external, value, span)) = context_decl {
            if let Some(value) = value.as_ref().filter(|_| !(external && setup.has_context)) {
                let vb = self.expr(value)?;
                let mut checked = vb;
                let va = checked.atom();
                let one = self.call_fn("exactly-one", FN_NS, vec![va], self.sp(&span))?;
                checked = checked.concat(one);
                let names = self.v.push_context();
                context_value = Some((checked, names));
            }
            let _ = type_;
        }

        // the globals, each initializer seeing the environment so far
        let mut steps: Vec<(ir::Name, ir::ExprS)> = Vec::new();
        let mut global_atoms: Vec<ir::AtomS> = vec![self.var_atom(&fns_ir, span0)];
        let env0 = ir::Expr::ArrayConstructor(ir::ArrayConstructor::Square(global_atoms.clone()));
        steps.push((env_ir.clone(), XSpanned::new(env0, span0)));
        for decl in &variable_decls {
            let Decl::Variable { name, type_, external, value, .. } = &decl.value else { continue };
            let span = self.sp(&decl.span);
            let init = if let Some(i) = setup.supplied.iter().position(|s| s == name).filter(|_| *external) {
                let p = supplied_ir.get(i).cloned().ok_or_else(|| cerr("XPST0008", "an external variable has no parameter", &decl.span))?;
                let a = self.var_atom(&p, span);
                self.atom_b(a)
            } else if let Some(value) = value {
                self.expr(value)?
            } else {
                self.empty(span)
            };
            let init = match type_ {
                Some(t) => {
                    let mut ib = init;
                    let ia = ib.atom();
                    let g = self.v.new_name();
                    let check = ir::Expr::InstanceOf(ir::InstanceOf { atom: self.var_atom(&g, span), sequence_type: t.clone() });
                    let mut cb = self.bind(check, span);
                    let ca = cb.atom();
                    let message = format!("the value of ${} does not match its declared type", name.local_name());
                    let fail = self.raise("XPTY0004", &message, span)?;
                    let gv = self.var_atom(&g, span);
                    let ok = XSpanned::new(ir::Expr::Atom(gv), span);
                    let iff = ir::Expr::If(ir::If { condition: ca, then: Box::new(ok), else_: Box::new(fail.expr()) });
                    let inner = cb.bind(self.v.new_binding(iff, span)).expr();
                    let let_ =
                        ir::Expr::Let(ir::Let { name: g, var_expr: Box::new(XSpanned::new(ir::Expr::Atom(ia), span)), return_expr: Box::new(inner) });
                    ib.bind(self.v.new_binding(let_, span))
                }
                None => init,
            };
            let g = self.v.new_name();
            steps.push((g.clone(), init.expr()));
            global_atoms.push(self.var_atom(&g, span));
            let env_i = ir::Expr::ArrayConstructor(ir::ArrayConstructor::Square(global_atoms.clone()));
            steps.push((env_ir.clone(), XSpanned::new(env_i, span)));
        }

        // the body
        let body = match &module.body {
            Some(b) => self.expr(b)?,
            None => self.empty(span0),
        };
        let mut inner = body.expr();
        for (name, value) in steps.into_iter().rev() {
            let span = inner.span;
            inner = XSpanned::new(ir::Expr::Let(ir::Let { name, var_expr: Box::new(value), return_expr: Box::new(inner) }), span);
        }
        if let Some((mut cb, names)) = context_value {
            self.v.pop_context();
            let ca = cb.atom();
            let span = inner.span;
            let map = self.v.new_binding(ir::Expr::Map(ir::Map { context_names: names, var_atom: ca, return_expr: Box::new(inner) }), span);
            inner = cb.bind(map).expr();
        }
        let span = inner.span;
        inner = XSpanned::new(ir::Expr::Let(ir::Let { name: fns_ir, var_expr: Box::new(fns_value), return_expr: Box::new(inner) }), span);
        self.v.pop_context();

        let mut params = vec![
            ir::Param { name: main_context.item, type_: None },
            ir::Param { name: main_context.position, type_: None },
            ir::Param { name: main_context.last, type_: None },
        ];
        for p in supplied_ir {
            params.push(ir::Param { name: p, type_: None });
        }
        let outer = ir::Expr::FunctionDefinition(ir::FunctionDefinition { params, return_type: None, body: Box::new(inner) });
        let span = self.sp(&module.body.as_ref().map_or(0..0, |b| b.span.clone()));
        Ok(Bindings::new(self.v.new_binding(outer, span)).expr())
    }

    fn function_definition(
        &mut self,
        params: &[Param],
        return_type: &Option<SequenceType>,
        body: &ExprS,
        declared: bool,
    ) -> C<ir::FunctionDefinition> {
        let saved = if declared { std::mem::take(&mut self.locals) } else { self.locals.clone() };
        let mut ir_params = Vec::new();
        if declared {
            ir_params.push(ir::Param { name: self.v.new_var_name(&self.env.clone()), type_: None });
        }
        for p in params {
            ir_params.push(ir::Param { name: self.v.new_var_name(&p.name), type_: p.type_.clone() });
            self.locals.push(p.name.clone());
        }
        self.v.push_absent_context();
        let b = self.expr(body);
        self.v.pop_context();
        self.locals = saved;
        Ok(ir::FunctionDefinition { params: ir_params, return_type: return_type.clone(), body: Box::new(b?.expr()) })
    }

    // ---- expressions ----

    pub(crate) fn expr(&mut self, e: &ExprS) -> C<Bindings> {
        let b = self.expr_inner(e)?;
        Ok(self.seal(b))
    }

    fn expr_inner(&mut self, e: &ExprS) -> C<Bindings> {
        let span = self.sp(&e.span);
        match &e.value {
            Expr::Sequence(items) => self.sequence(items, span),
            Expr::Literal(l) => {
                let c = match l {
                    Literal::Integer(i) => ir::Const::Integer(i.clone()),
                    Literal::Decimal(d) => ir::Const::Decimal(*d),
                    Literal::Double(d) => ir::Const::Double(*d),
                    Literal::String(s) => ir::Const::String(s.clone()),
                };
                let a = self.konst(c, span);
                Ok(self.atom_b(a))
            }
            Expr::VarRef(name) => self.var_ref(name, &e.span),
            Expr::ContextItem => self.v.context_item(span).map_err(from_xee),
            Expr::Root => {
                let mut c = self.v.context_item(span).map_err(from_xee)?;
                let ca = c.atom();
                let mut root = self.call_fn("root", FN_NS, vec![ca], span)?;
                let ra = root.atom();
                let doc = item_type(xast::ItemType::KindTest(xast::KindTest::Document(None)), xast::Occurrence::One);
                let t = self.v.new_binding(ir::Expr::Treat(ir::Treat { atom: ra, sequence_type: doc }), span);
                Ok(c.concat(root).bind(t))
            }
            Expr::FunctionCall { name, args } => self.function_call(name, args, &e.span),
            Expr::NamedFunctionRef { name, arity } => self.named_function_ref(name, usize::from(*arity), &e.span),
            Expr::InlineFunction { params, return_type, body, .. } => {
                let def = self.function_definition(params, return_type, body, false)?;
                Ok(self.bind(ir::Expr::FunctionDefinition(def), span))
            }
            Expr::DynamicCall { function, args } => {
                let mut fb = self.expr(function)?;
                let fa = fb.atom();
                let call = self.with_args(args, span, &mut |s, atoms| Ok(s.call_atom(fa.clone(), atoms, span)))?;
                Ok(fb.concat(call))
            }
            Expr::Filter { base, predicate } => {
                let mut bb = self.expr(base)?;
                let ba = bb.atom();
                let names = self.v.push_context();
                let pb = self.expr(predicate);
                self.v.pop_context();
                let f =
                    self.v.new_binding(ir::Expr::Filter(ir::Filter { context_names: names, var_atom: ba, return_expr: Box::new(pb?.expr()) }), span);
                Ok(bb.bind(f))
            }
            Expr::Lookup { base, key } => {
                let mut bb = self.expr(base)?;
                let ba = bb.atom();
                let item = self.v.new_name();
                let names = self.v.explicit_context_names(item.clone());
                let lookup = self.lookup(self.var_atom(&item, span), key, span)?;
                let m = self.v.new_binding(ir::Expr::Map(ir::Map { context_names: names, var_atom: ba, return_expr: Box::new(lookup.expr()) }), span);
                Ok(bb.bind(m))
            }
            Expr::UnaryLookup(key) => {
                let mut c = self.v.context_item(span).map_err(from_xee)?;
                let ca = c.atom();
                let l = self.lookup(ca, key, span)?;
                Ok(c.concat(l))
            }
            Expr::Path { left, right } => {
                let mut lb = self.expr(left)?;
                let la = lb.atom();
                let names = self.v.push_context();
                let rb = self.expr(right);
                self.v.pop_context();
                let m = XSpanned::new(ir::Expr::Map(ir::Map { context_names: names, var_atom: la, return_expr: Box::new(rb?.expr()) }), span);
                let d = self.v.new_binding(ir::Expr::Deduplicate(Box::new(m)), span);
                Ok(lb.bind(d))
            }
            Expr::AxisStep { axis, test, predicates } => {
                if matches!(axis, xast::Axis::Namespace) {
                    return Err(cerr("XQST0134", "the namespace axis is not supported", &e.span));
                }
                let mut c = self.v.context_item(span).map_err(from_xee)?;
                let ca = c.atom();
                let step = xee_interpreter::xml::Step { axis: axis.clone(), node_test: test.clone() };
                let s = self.v.new_binding(ir::Expr::Step(ir::Step { step, context: ca }), span);
                let mut b = Bindings::new(s);
                for p in predicates {
                    let ba = b.atom();
                    let names = self.v.push_context();
                    let pb = self.expr(p);
                    self.v.pop_context();
                    let pspan = self.sp(&p.span);
                    let f = self
                        .v
                        .new_binding(ir::Expr::Filter(ir::Filter { context_names: names, var_atom: ba, return_expr: Box::new(pb?.expr()) }), pspan);
                    b = b.bind(f);
                }
                Ok(c.concat(b))
            }
            Expr::Binary { op, left, right } => {
                let lb = self.expr(left)?;
                let rb = self.expr(right)?;
                Ok(self.binary(lb, binop(*op), rb, span))
            }
            Expr::Unary { minus, expr } => {
                let mut b = self.expr(expr)?;
                let a = b.atom();
                let op = if *minus { xast::UnaryOperator::Minus } else { xast::UnaryOperator::Plus };
                let u = self.v.new_binding(ir::Expr::Unary(ir::Unary { op, atom: a }), span);
                Ok(b.bind(u))
            }
            Expr::SimpleMap { left, right } => {
                let mut lb = self.expr(left)?;
                let la = lb.atom();
                let names = self.v.push_context();
                let rb = self.expr(right);
                self.v.pop_context();
                let m = self.v.new_binding(ir::Expr::Map(ir::Map { context_names: names, var_atom: la, return_expr: Box::new(rb?.expr()) }), span);
                Ok(lb.bind(m))
            }
            Expr::Arrow { base, function, args } => {
                let mut all = vec![Arg::Expr((**base).clone())];
                all.extend(args.iter().cloned());
                match function.as_ref() {
                    ArrowTarget::Name(name) => self.function_call(name, &all, &e.span),
                    ArrowTarget::Expr(f) => {
                        let call = Spanned::new(Expr::DynamicCall { function: Box::new(f.clone()), args: all }, e.span.clone());
                        self.expr_inner(&call)
                    }
                }
            }
            Expr::InstanceOf { expr, type_ } => {
                let mut b = self.expr(expr)?;
                let a = b.atom();
                let i = self.v.new_binding(ir::Expr::InstanceOf(ir::InstanceOf { atom: a, sequence_type: type_.clone() }), span);
                Ok(b.bind(i))
            }
            Expr::Treat { expr, type_ } => {
                let mut b = self.expr(expr)?;
                let a = b.atom();
                let t = self.v.new_binding(ir::Expr::Treat(ir::Treat { atom: a, sequence_type: type_.clone() }), span);
                Ok(b.bind(t))
            }
            Expr::Cast { expr, type_ } | Expr::Castable { expr, type_ } => {
                let xs = cast_type(type_, &e.span)?;
                let mut b = self.expr(expr)?;
                let a = b.atom();
                let c = if matches!(e.value, Expr::Cast { .. }) {
                    ir::Expr::Cast(ir::Cast { atom: a, xs, empty_sequence_allowed: type_.optional })
                } else {
                    ir::Expr::Castable(ir::Castable { atom: a, xs, empty_sequence_allowed: type_.optional })
                };
                let c = self.v.new_binding(c, span);
                Ok(b.bind(c))
            }
            Expr::If { condition, then, else_ } => {
                let mut cb = self.expr(condition)?;
                let ca = cb.atom();
                let tb = self.expr(then)?;
                let eb = self.expr(else_)?;
                let i = self.v.new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(tb.expr()), else_: Box::new(eb.expr()) }), span);
                Ok(cb.bind(i))
            }
            Expr::Switch { operand, cases, default } => self.switch(operand, cases, default, span),
            Expr::Typeswitch { operand, cases, default_var, default } => self.typeswitch(operand, cases, default_var, default, span),
            Expr::Try { body, catches } => self.try_catch(body, catches, span),
            Expr::Quantified { every, bindings, satisfies } => self.quantified(*every, bindings, satisfies, span),
            Expr::Flwor { clauses, ret } => self.flwor(clauses, ret),
            Expr::MapConstructor(entries) => {
                let mut b = Bindings::empty();
                let mut members = Vec::new();
                for (k, val) in entries {
                    let mut kb = self.expr(k)?;
                    let ka = kb.atom();
                    let mut vb = self.expr(val)?;
                    let va = vb.atom();
                    members.push((ka, va));
                    b = b.concat(kb).concat(vb);
                }
                let m = self.v.new_binding(ir::Expr::MapConstructor(ir::MapConstructor { members }), span);
                Ok(b.bind(m))
            }
            Expr::SquareArray(members) => {
                let mut b = Bindings::empty();
                let mut atoms = Vec::new();
                for m in members {
                    let mut mb = self.expr(m)?;
                    atoms.push(mb.atom());
                    b = b.concat(mb);
                }
                let a = self.v.new_binding(ir::Expr::ArrayConstructor(ir::ArrayConstructor::Square(atoms)), span);
                Ok(b.bind(a))
            }
            Expr::CurlyArray(content) => {
                let mut b = match content {
                    Some(c) => self.expr(c)?,
                    None => self.empty(span),
                };
                let a = b.atom();
                let arr = self.v.new_binding(ir::Expr::ArrayConstructor(ir::ArrayConstructor::Curly(a)), span);
                Ok(b.bind(arr))
            }
            Expr::StringConstructor(parts) => {
                let mut b = Bindings::empty();
                let mut atoms = Vec::new();
                for p in parts {
                    match p {
                        StringPart::Text(t) => atoms.push(self.string(t, span)),
                        StringPart::Expr(Some(x)) => {
                            let mut sb = self.joined(x, " ")?;
                            atoms.push(sb.atom());
                            b = b.concat(sb);
                        }
                        StringPart::Expr(None) => {}
                    }
                }
                let s = self.string_join(atoms, "", span)?;
                Ok(b.concat(s))
            }
            Expr::Validate(_) => Err(cerr("XQST0075", "validate is not supported: the engine is not schema-aware", &e.span)),
            Expr::DirElement { name, namespaces, attributes, content } => self.direct_element(name, namespaces, attributes, content, span),
            Expr::DirComment(text) => {
                let a = self.string(text, span);
                Ok(self.bind(ir::Expr::XmlComment(ir::XmlComment { value: a }), span))
            }
            Expr::DirPi { target, content } => {
                let t = self.string(target, span);
                let c = self.string(content, span);
                Ok(self.bind(ir::Expr::XmlProcessingInstruction(ir::XmlProcessingInstruction { target: t, content: c }), span))
            }
            Expr::CompDocument(content) => {
                let mut d = self.bind(ir::Expr::XmlDocument(ir::XmlRoot {}), span);
                let da = d.atom();
                let mut cb = self.expr(content)?;
                let ca = cb.atom();
                let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: da, child: ca }), span);
                Ok(d.concat(cb).bind(append))
            }
            Expr::CompElement { name, content } => {
                let mut nb = self.comp_name(name, true, span)?;
                let na = nb.atom();
                let el = self.v.new_binding(ir::Expr::XmlElement(ir::XmlElement { name: na }), span);
                let mut b = nb.bind(el);
                if let CompName::Name(n) = name {
                    b = self.prefix_namespace(b, n, &[], span);
                }
                match content {
                    Some(c) => {
                        let ea = b.atom();
                        let mut cb = self.expr(c)?;
                        let ca = cb.atom();
                        let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: ea, child: ca }), span);
                        Ok(b.concat(cb).bind(append))
                    }
                    None => Ok(b),
                }
            }
            Expr::CompAttribute { name, content } => {
                if let CompName::Name(n) = name
                    && n.local_name() == "xmlns"
                    && n.namespace().is_empty()
                {
                    return Err(cerr("XQDY0044", "an attribute cannot be named xmlns", &e.span));
                }
                let mut nb = self.comp_name(name, false, span)?;
                let na = nb.atom();
                let mut vb = match content {
                    Some(c) => self.joined(c, " ")?,
                    None => {
                        let a = self.string("", span);
                        self.atom_b(a)
                    }
                };
                let va = vb.atom();
                let at = self.v.new_binding(ir::Expr::XmlAttribute(ir::XmlAttribute { name: na, value: va }), span);
                Ok(nb.concat(vb).bind(at))
            }
            Expr::CompNamespace { prefix, uri } => {
                let mut pb = match prefix {
                    CompName::NcName(p) => {
                        let a = self.string(p, span);
                        self.atom_b(a)
                    }
                    CompName::Expr(x) => {
                        let mut xb = self.expr(x)?;
                        let xa = xb.atom();
                        let c = self.prelude_call("ns-prefix", vec![xa], span)?;
                        xb.concat(c)
                    }
                    CompName::Name(n) => {
                        let a = self.string(n.local_name(), span);
                        self.atom_b(a)
                    }
                };
                let pa = pb.atom();
                let mut ub = match uri {
                    Some(u) => self.joined(u, "")?,
                    None => {
                        let a = self.string("", span);
                        self.atom_b(a)
                    }
                };
                let ua = ub.atom();
                let mut checked = self.prelude_call("ns-uri", vec![pa.clone(), ua], span)?;
                let ca = checked.atom();
                let n = self.v.new_binding(ir::Expr::XmlNamespace(ir::XmlNamespace { prefix: pa, namespace: ca }), span);
                Ok(pb.concat(ub).concat(checked).bind(n))
            }
            Expr::CompText(content) => {
                let mut cb = self.expr(content)?;
                let ca = cb.atom();
                let mut data = self.call_fn("data", FN_NS, vec![ca], span)?;
                let da = data.atom();
                let mut exists = self.call_fn("exists", FN_NS, vec![da.clone()], span)?;
                let ea = exists.atom();
                let sep = self.string(" ", span);
                let mut joined = self.call_fn("string-join", FN_NS, vec![da, sep], span)?;
                let ja = joined.atom();
                let text = self.v.new_binding(ir::Expr::XmlText(ir::XmlText { value: ja }), span);
                let then = joined.bind(text).expr();
                let iff =
                    self.v.new_binding(ir::Expr::If(ir::If { condition: ea, then: Box::new(then), else_: Box::new(self.empty_expr(span)) }), span);
                Ok(cb.concat(data).concat(exists).bind(iff))
            }
            Expr::CompComment(content) => {
                let mut jb = self.joined(content, " ")?;
                let ja = jb.atom();
                let mut checked = self.prelude_call("comment-text", vec![ja], span)?;
                let ca = checked.atom();
                let c = self.v.new_binding(ir::Expr::XmlComment(ir::XmlComment { value: ca }), span);
                Ok(jb.concat(checked).bind(c))
            }
            Expr::CompPi { target, content } => {
                let mut tb = match target {
                    CompName::NcName(t) => {
                        if t.eq_ignore_ascii_case("xml") {
                            return Err(cerr("XQDY0064", "a processing instruction cannot be named 'xml'", &e.span));
                        }
                        let a = self.string(t, span);
                        self.atom_b(a)
                    }
                    CompName::Name(n) => {
                        let a = self.string(n.local_name(), span);
                        self.atom_b(a)
                    }
                    CompName::Expr(x) => {
                        let mut xb = self.expr(x)?;
                        let xa = xb.atom();
                        let c = self.prelude_call("pi-target", vec![xa], span)?;
                        xb.concat(c)
                    }
                };
                let ta = tb.atom();
                let mut cb = match content {
                    Some(c) => self.joined(c, " ")?,
                    None => {
                        let a = self.string("", span);
                        self.atom_b(a)
                    }
                };
                let ca = cb.atom();
                let mut checked = self.prelude_call("pi-content", vec![ca], span)?;
                let ka = checked.atom();
                let pi = self.v.new_binding(ir::Expr::XmlProcessingInstruction(ir::XmlProcessingInstruction { target: ta, content: ka }), span);
                Ok(tb.concat(cb).concat(checked).bind(pi))
            }
        }
    }

    /// A comma sequence, split in halves so that a long one does not nest deeply.
    fn sequence(&mut self, items: &[ExprS], span: XSpan) -> C<Bindings> {
        match items {
            [] => Ok(self.empty(span)),
            [one] => self.expr(one),
            _ => {
                let mid = items.len() / 2;
                let (l, r) = items.split_at(mid);
                let lb = self.sequence(l, span)?;
                let lb = self.seal(lb);
                let rb = self.sequence(r, span)?;
                let rb = self.seal(rb);
                Ok(self.binary(lb, xast::BinaryOperator::Comma, rb, span))
            }
        }
    }

    /// `string-join(data(e), sep)`
    fn joined(&mut self, e: &ExprS, sep: &str) -> C<Bindings> {
        let span = self.sp(&e.span);
        let mut b = self.expr(e)?;
        let a = b.atom();
        let mut data = self.call_fn("data", FN_NS, vec![a], span)?;
        let da = data.atom();
        let s = self.string(sep, span);
        let j = self.call_fn("string-join", FN_NS, vec![da, s], span)?;
        Ok(b.concat(data).concat(j))
    }

    fn string_join(&mut self, atoms: Vec<ir::AtomS>, sep: &str, span: XSpan) -> C<Bindings> {
        let mut seq = self.sequence_of(atoms, span);
        let sa = seq.atom();
        let s = self.string(sep, span);
        let j = self.call_fn("string-join", FN_NS, vec![sa, s], span)?;
        Ok(seq.concat(j))
    }

    fn var_ref(&mut self, name: &Name, span: &Span) -> C<Bindings> {
        let xspan = self.sp(span);
        if self.locals.iter().rev().any(|l| l == name) {
            return self.v.var_ref(name, xspan).map_err(from_xee);
        }
        if self.missing.contains(name) {
            let message = format!("no value was given for the external variable ${}", name.local_name());
            return self.raise("XPDY0002", &message, xspan);
        }
        if let Some(i) = self.globals.iter().position(|g| g == name) {
            return self.env_member(i.saturating_add(2), xspan);
        }
        Err(cerr("XPST0008", format!("the variable ${} is not declared", display_name(name)), span))
    }

    /// Compile arguments; with `?` placeholders, the call becomes a function of the missing arguments (a partial application),
    /// the other arguments evaluated now.
    fn with_args(&mut self, args: &[Arg], span: XSpan, call: &mut dyn FnMut(&mut Self, Vec<ir::AtomS>) -> C<Bindings>) -> C<Bindings> {
        let mut b = Bindings::empty();
        let mut atoms: Vec<Option<ir::AtomS>> = Vec::new();
        for a in args {
            match a {
                Arg::Expr(e) => {
                    let mut eb = self.expr(e)?;
                    atoms.push(Some(eb.atom()));
                    b = b.concat(eb);
                }
                Arg::Placeholder(_) => atoms.push(None),
            }
        }
        if atoms.iter().all(Option::is_some) {
            let r = call(self, atoms.into_iter().flatten().collect())?;
            return Ok(b.concat(r));
        }
        let mut params = Vec::new();
        let mut filled = Vec::new();
        for a in atoms {
            match a {
                Some(a) => filled.push(a),
                None => {
                    let p = self.v.new_name();
                    filled.push(self.var_atom(&p, span));
                    params.push(ir::Param { name: p, type_: None });
                }
            }
        }
        let body = call(self, filled)?;
        let def = ir::Expr::FunctionDefinition(ir::FunctionDefinition { params, return_type: None, body: Box::new(body.expr()) });
        let d = self.v.new_binding(def, span);
        Ok(b.bind(d))
    }

    fn function_call(&mut self, name: &Name, args: &[Arg], span: &Span) -> C<Bindings> {
        let xspan = self.sp(span);
        let arity = args.len();
        if let Some(index) = self.functions.iter().position(|(n, a)| n == name && *a == arity) {
            return self.with_args(args, xspan, &mut |s, atoms| s.call_user(index, atoms, xspan));
        }
        if name.namespace() == FN_NS && arity == 0 && args.is_empty() {
            if name.local_name() == "position" {
                return self.v.fn_position(xspan).map_err(from_xee);
            }
            if name.local_name() == "last" {
                return self.v.fn_last(xspan).map_err(from_xee);
            }
        }
        if name.namespace() == FN_NS && name.local_name().starts_with("xmlcraft-") {
            return Err(cerr("XPST0017", format!("there is no function {}#{arity}", display_name(name)), span));
        }
        let arity8 = u8::try_from(arity).map_err(|_| cerr("XPST0017", "too many arguments", span))?;
        let Some(id) = self.sc.function_id_by_name(name, arity8) else {
            return Err(cerr("XPST0017", format!("there is no function {}#{arity}", display_name(name)), span));
        };
        self.with_args(args, xspan, &mut |s, atoms| {
            let fref = ir::Atom::Const(ir::Const::StaticFunctionReference(id, s.v.current_context_names()));
            let fa = XSpanned::new(fref, xspan);
            Ok(s.call_atom(fa, atoms, xspan))
        })
    }

    fn named_function_ref(&mut self, name: &Name, arity: usize, span: &Span) -> C<Bindings> {
        let xspan = self.sp(span);
        if self.functions.iter().any(|(n, a)| n == name && *a == arity) {
            let args: Vec<Arg> = (0..arity).map(|_| Arg::Placeholder(span.clone())).collect();
            return self.function_call(name, &args, span);
        }
        let arity8 = u8::try_from(arity).map_err(|_| cerr("XPST0017", "too many arguments", span))?;
        let Some(id) = self.sc.function_id_by_name(name, arity8) else {
            return Err(cerr("XPST0017", format!("there is no function {}#{arity}", display_name(name)), span));
        };
        let fref = ir::Atom::Const(ir::Const::StaticFunctionReference(id, self.v.current_context_names()));
        Ok(self.atom_b(XSpanned::new(fref, xspan)))
    }

    fn lookup(&mut self, atom: ir::AtomS, key: &Key, span: XSpan) -> C<Bindings> {
        let arg = match key {
            Key::Star => return Ok(self.bind(ir::Expr::WildcardLookup(ir::WildcardLookup { atom }), span)),
            Key::Name(n) => self.atom_b(self.string(n, span)),
            Key::Integer(i) => self.atom_b(self.konst(ir::Const::Integer(i.clone()), span)),
            Key::Expr(e) => self.expr(e)?,
        };
        let mut arg = arg;
        let aa = arg.atom();
        let l = self.v.new_binding(ir::Expr::Lookup(ir::Lookup { atom, arg_atom: aa }), span);
        Ok(arg.bind(l))
    }

    fn quantified(&mut self, every: bool, bindings: &[(VarBinding, ExprS)], satisfies: &ExprS, span: XSpan) -> C<Bindings> {
        let Some(((var, e), rest)) = bindings.split_first() else { return self.expr(satisfies) };
        let mut sb = self.expr(e)?;
        let sa = sb.atom();
        let name = self.v.new_var_name(&var.name);
        let names = self.v.explicit_context_names(name);
        self.locals.push(var.name.clone());
        let inner = self.type_check(var, &mut |s| s.quantified(every, rest, satisfies, span));
        self.locals.pop();
        let quantifier = if every { ir::Quantifier::Every } else { ir::Quantifier::Some };
        let q = self.v.new_binding(
            ir::Expr::Quantified(ir::Quantified { quantifier, context_names: names, var_atom: sa, satisifies_expr: Box::new(inner?.expr()) }),
            span,
        );
        Ok(sb.bind(q))
    }

    fn switch(&mut self, operand: &ExprS, cases: &[(Vec<ExprS>, ExprS)], default: &ExprS, span: XSpan) -> C<Bindings> {
        let mut ob = self.expr(operand)?;
        let oa = ob.atom();
        let mut key = self.prelude_call("switch-key", vec![oa], span)?;
        let ka = key.atom();
        let mut otherwise = self.expr(default)?;
        for (values, ret) in cases.iter().rev() {
            let mut cond: Option<Bindings> = None;
            for value in values {
                let mut vb = self.expr(value)?;
                let va = vb.atom();
                let m = self.prelude_call("switch-match", vec![ka.clone(), va], span)?;
                let test = vb.concat(m);
                cond = Some(match cond {
                    None => test,
                    Some(c) => self.binary(c, xast::BinaryOperator::Or, test, span),
                });
            }
            let Some(mut c) = cond else { continue };
            let ca = c.atom();
            let rb = self.expr(ret)?;
            let i = self.v.new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(rb.expr()), else_: Box::new(otherwise.expr()) }), span);
            otherwise = self.seal(c.bind(i));
        }
        Ok(ob.concat(key).concat(otherwise))
    }

    fn typeswitch(&mut self, operand: &ExprS, cases: &[TypeCase], default_var: &Option<VarBinding>, default: &ExprS, span: XSpan) -> C<Bindings> {
        let mut ob = self.expr(operand)?;
        let oa = ob.atom();
        let bind_var = |s: &mut Self, var: &Option<VarBinding>, ret: &ExprS| -> C<Bindings> {
            match var {
                Some(v) => {
                    let name = s.v.new_var_name(&v.name);
                    s.locals.push(v.name.clone());
                    let rb = s.expr(ret);
                    s.locals.pop();
                    let l = ir::Expr::Let(ir::Let {
                        name,
                        var_expr: Box::new(XSpanned::new(ir::Expr::Atom(oa.clone()), span)),
                        return_expr: Box::new(rb?.expr()),
                    });
                    Ok(s.bind(l, span))
                }
                None => s.expr(ret),
            }
        };
        let mut otherwise = bind_var(self, default_var, default)?;
        for case in cases.iter().rev() {
            let mut cond: Option<Bindings> = None;
            for t in &case.types {
                let test = self.bind(ir::Expr::InstanceOf(ir::InstanceOf { atom: oa.clone(), sequence_type: t.clone() }), span);
                cond = Some(match cond {
                    None => test,
                    Some(c) => self.binary(c, xast::BinaryOperator::Or, test, span),
                });
            }
            let Some(mut c) = cond else { continue };
            let ca = c.atom();
            let rb = bind_var(self, &case.var, &case.ret)?;
            let i = self.v.new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(rb.expr()), else_: Box::new(otherwise.expr()) }), span);
            otherwise = self.seal(c.bind(i));
        }
        Ok(ob.concat(otherwise))
    }

    fn try_catch(&mut self, body: &ExprS, catches: &[Catch], span: XSpan) -> C<Bindings> {
        // the body: a function of no arguments that keeps the context of the `try`
        let bb = self.expr(body)?;
        let body_fn = ir::Expr::FunctionDefinition(ir::FunctionDefinition { params: Vec::new(), return_type: None, body: Box::new(bb.expr()) });
        let mut body_b = self.bind(body_fn, span);
        let body_atom = body_b.atom();
        // the handler: ($err:code, $err:description, $err:value), choosing the clause
        let code = err_name("code");
        let description = err_name("description");
        let value = err_name("value");
        let params = vec![
            ir::Param { name: self.v.new_var_name(&code), type_: None },
            ir::Param { name: self.v.new_var_name(&description), type_: None },
            ir::Param { name: self.v.new_var_name(&value), type_: None },
        ];
        let extra = [err_name("module"), err_name("line-number"), err_name("column-number"), err_name("additional")];
        let extra_ir: Vec<ir::Name> = extra.iter().map(|n| self.v.new_var_name(n)).collect();
        let pushed = 3 + extra.len();
        self.locals.extend([code.clone(), description.clone(), value.clone()]);
        self.locals.extend(extra.iter().cloned());
        let handler = self.catch_clauses(catches, &code, &description, span);
        for _ in 0..pushed {
            self.locals.pop();
        }
        let mut handler = handler?.expr();
        for n in extra_ir.into_iter().rev() {
            let hs = handler.span;
            handler =
                XSpanned::new(ir::Expr::Let(ir::Let { name: n, var_expr: Box::new(self.empty_expr(span)), return_expr: Box::new(handler) }), hs);
        }
        let handler_fn = ir::Expr::FunctionDefinition(ir::FunctionDefinition { params, return_type: None, body: Box::new(handler) });
        let mut handler_b = self.bind(handler_fn, span);
        let handler_atom = handler_b.atom();
        let call = self.call_fn("xmlcraft-try", FN_NS, vec![body_atom, handler_atom], span)?;
        Ok(body_b.concat(handler_b).concat(call))
    }

    fn catch_clauses(&mut self, catches: &[Catch], code: &Name, description: &Name, span: XSpan) -> C<Bindings> {
        let Some((first, rest)) = catches.split_first() else {
            // no clause matched: raise the error again
            let mut c = self.v.var_ref(code, span).map_err(from_xee)?;
            let ca = c.atom();
            let mut d = self.v.var_ref(description, span).map_err(from_xee)?;
            let da = d.atom();
            let e = self.call_fn("error", FN_NS, vec![ca, da], span)?;
            return Ok(c.concat(d).concat(e));
        };
        let cspan = self.sp(&first.span);
        let body = self.expr(&first.body)?;
        if first.tests.iter().any(|t| matches!(t, xast::NameTest::Star)) {
            return Ok(body);
        }
        let mut cond: Option<Bindings> = None;
        for t in &first.tests {
            let mut c = self.v.var_ref(code, cspan).map_err(from_xee)?;
            let ca = c.atom();
            let test = match t {
                xast::NameTest::Name(n) => {
                    let ns = self.string(n.value.namespace(), cspan);
                    let local = self.string(n.value.local_name(), cspan);
                    let nsb = self.call_fn("namespace-uri-from-QName", FN_NS, vec![ca.clone()], cspan)?;
                    let nse = self.atom_b(ns);
                    let ns_eq = self.binary(nsb, xast::BinaryOperator::ValueEq, nse, cspan);
                    let lb = self.call_fn("local-name-from-QName", FN_NS, vec![ca], cspan)?;
                    let le = self.atom_b(local);
                    let l_eq = self.binary(lb, xast::BinaryOperator::ValueEq, le, cspan);
                    self.binary(ns_eq, xast::BinaryOperator::And, l_eq, cspan)
                }
                xast::NameTest::LocalName(l) => {
                    let local = self.string(l, cspan);
                    let lb = self.call_fn("local-name-from-QName", FN_NS, vec![ca], cspan)?;
                    let le = self.atom_b(local);
                    self.binary(lb, xast::BinaryOperator::ValueEq, le, cspan)
                }
                xast::NameTest::Namespace(u) => {
                    let ns = self.string(u, cspan);
                    let nsb = self.call_fn("namespace-uri-from-QName", FN_NS, vec![ca], cspan)?;
                    let nse = self.atom_b(ns);
                    self.binary(nsb, xast::BinaryOperator::ValueEq, nse, cspan)
                }
                xast::NameTest::Star => self.boolean(true, cspan)?,
            };
            let test = c.concat(test);
            cond = Some(match cond {
                None => test,
                Some(prev) => self.binary(prev, xast::BinaryOperator::Or, test, cspan),
            });
        }
        let Some(mut cond) = cond else { return Ok(body) };
        let ca = cond.atom();
        let otherwise = self.catch_clauses(rest, code, description, span)?;
        let i = self.v.new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(body.expr()), else_: Box::new(otherwise.expr()) }), cspan);
        Ok(cond.bind(i))
    }

    // ---- FLWOR ----

    fn flwor(&mut self, clauses: &[Spanned<Clause>], ret: &ExprS) -> C<Bindings> {
        let k = clauses.iter().position(|c| matches!(c.value, Clause::OrderBy { .. } | Clause::GroupBy(_) | Clause::Count(_)));
        let Some(k) = k else {
            return self.clauses(clauses, &mut |s| s.expr(ret));
        };
        let (prefix, rest) = clauses.split_at(k);
        let Some((clause, after)) = rest.split_first() else { return self.clauses(clauses, &mut |s| s.expr(ret)) };
        let span = self.sp(&clause.span);
        let vars = bound_vars(prefix);
        let n = vars.len();
        match &clause.value {
            Clause::Count(var) => {
                let tuples = self.clauses(prefix, &mut |s| s.tuple(&vars, &[], span))?;
                self.for_tuples(tuples, &vars, &[], Some(var), span, &mut |s| s.flwor(after, ret))
            }
            Clause::OrderBy { specs, .. } => {
                let keys: Vec<&ExprS> = specs.iter().map(|s| &s.expr).collect();
                let mut tuples = self.clauses(prefix, &mut |s| s.tuple(&vars, &keys, span))?;
                for (j, spec) in specs.iter().enumerate().rev() {
                    let ta = tuples.atom();
                    let index = self.int(n.saturating_add(j).saturating_add(1), span);
                    let desc = self.boolean(spec.descending, span)?;
                    let eg = self.boolean(spec.empty_greatest.unwrap_or(self.empty_greatest), span)?;
                    let (mut db, mut gb) = (desc, eg);
                    let da = db.atom();
                    let ga = gb.atom();
                    let coll = match &spec.collation {
                        Some(c) => self.string(c, span),
                        None => self.konst(ir::Const::EmptySequence, span),
                    };
                    let call = self.prelude_call("sort-pass", vec![ta, index, da, ga, coll], span)?;
                    tuples = self.seal(tuples.concat(db).concat(gb).concat(call));
                }
                self.for_tuples(tuples, &vars, &[], None, span, &mut |s| s.flwor(after, ret))
            }
            Clause::GroupBy(specs) => {
                let mut key_exprs: Vec<ExprS> = Vec::new();
                for spec in specs {
                    match &spec.expr {
                        Some(e) => key_exprs.push(e.clone()),
                        None => {
                            if !vars.contains(&spec.var.name) && !self.locals.contains(&spec.var.name) {
                                return Err(cerr(
                                    "XQST0094",
                                    format!("${} is not bound by the FLWOR expression", spec.var.name.local_name()),
                                    &spec.var.span,
                                ));
                            }
                            key_exprs.push(Spanned::new(Expr::VarRef(spec.var.name.clone()), spec.var.span.clone()));
                        }
                    }
                }
                let keys: Vec<&ExprS> = key_exprs.iter().collect();
                let mut tuples = self.clauses(prefix, &mut |s| s.tuple(&vars, &keys, span))?;
                let ta = tuples.atom();
                let colls: Vec<ir::AtomS> = specs.iter().map(|s| self.string(s.collation.as_deref().unwrap_or(""), span)).collect();
                let mut cb = self.sequence_of(colls, span);
                let ca = cb.atom();
                let nn = self.int(n, span);
                let m = self.int(specs.len(), span);
                let call = self.prelude_call("group", vec![ta, nn, m, ca], span)?;
                let grouped = self.seal(tuples.concat(cb).concat(call));
                let group_vars: Vec<Name> = specs.iter().map(|s| s.var.name.clone()).collect();
                let typed: Vec<&VarBinding> = specs.iter().map(|s| &s.var).collect();
                self.for_tuples_typed(grouped, &vars, &group_vars, &typed, span, &mut |s| s.flwor(after, ret))
            }
            _ => self.clauses(clauses, &mut |s| s.expr(ret)),
        }
    }

    fn boolean(&mut self, b: bool, span: XSpan) -> C<Bindings> {
        self.call_fn(if b { "true" } else { "false" }, FN_NS, Vec::new(), span)
    }

    /// `[$v1, …, $vn, key1, …]`, the keys made single atomic values.
    fn tuple(&mut self, vars: &[Name], keys: &[&ExprS], span: XSpan) -> C<Bindings> {
        let mut b = Bindings::empty();
        let mut atoms = Vec::new();
        for v in vars {
            let mut vb = self.v.var_ref(v, span).map_err(from_xee)?;
            atoms.push(vb.atom());
            b = b.concat(vb);
        }
        for k in keys {
            let kspan = self.sp(&k.span);
            let mut kb = self.expr(k)?;
            let ka = kb.atom();
            let mut c = self.prelude_call("key", vec![ka], kspan)?;
            atoms.push(c.atom());
            b = b.concat(kb).concat(c);
        }
        let a = self.v.new_binding(ir::Expr::ArrayConstructor(ir::ArrayConstructor::Square(atoms)), span);
        Ok(b.bind(a))
    }

    fn for_tuples(
        &mut self,
        tuples: Bindings,
        vars: &[Name],
        group_vars: &[Name],
        count: Option<&VarBinding>,
        span: XSpan,
        rest: &mut dyn FnMut(&mut Self) -> C<Bindings>,
    ) -> C<Bindings> {
        let mut tuples = tuples;
        let ta = tuples.atom();
        let t = self.v.new_name();
        let position = match count {
            Some(c) => self.v.new_var_name(&c.name),
            None => self.v.new_name(),
        };
        let names = ir::ContextNames { item: t.clone(), position, last: self.v.new_name() };
        let all: Vec<&Name> = vars.iter().chain(group_vars.iter()).collect();
        let pushed = all.len() + usize::from(count.is_some());
        self.locals.extend(all.iter().map(|n| (*n).clone()));
        if let Some(c) = count {
            self.locals.push(c.name.clone());
        }
        let body = rest(self);
        for _ in 0..pushed {
            self.locals.pop();
        }
        let mut body = body?.expr();
        for (i, v) in all.iter().enumerate().rev() {
            let name = self.v.new_var_name(v);
            let ti = self.var_atom(&t, span);
            let index = self.int(i.saturating_add(1), span);
            let get = XSpanned::new(ir::Expr::FunctionCall(ir::FunctionCall { atom: ti, args: vec![index] }), span);
            let bs = body.span;
            body = XSpanned::new(ir::Expr::Let(ir::Let { name, var_expr: Box::new(get), return_expr: Box::new(body) }), bs);
        }
        let m = self.v.new_binding(ir::Expr::Map(ir::Map { context_names: names, var_atom: ta, return_expr: Box::new(body) }), span);
        Ok(tuples.bind(m))
    }

    /// After `group by`: the non-grouping variables are bound to their values joined over the group, the grouping variables to
    /// the keys (checked against their declared types).
    fn for_tuples_typed(
        &mut self,
        tuples: Bindings,
        vars: &[Name],
        group_vars: &[Name],
        typed: &[&VarBinding],
        span: XSpan,
        rest: &mut dyn FnMut(&mut Self) -> C<Bindings>,
    ) -> C<Bindings> {
        let typed: Vec<VarBinding> = typed.iter().map(|v| (*v).clone()).collect();
        self.for_tuples(tuples, vars, group_vars, None, span, &mut |s| s.checks(&typed, rest))
    }

    fn checks(&mut self, vars: &[VarBinding], rest: &mut dyn FnMut(&mut Self) -> C<Bindings>) -> C<Bindings> {
        let Some((first, others)) = vars.split_first() else { return rest(self) };
        self.type_check(first, &mut |s| s.checks(others, rest))
    }

    fn clauses(&mut self, clauses: &[Spanned<Clause>], tail: &mut dyn FnMut(&mut Self) -> C<Bindings>) -> C<Bindings> {
        let Some((first, rest)) = clauses.split_first() else { return tail(self) };
        let span = self.sp(&first.span);
        match &first.value {
            Clause::For { var, allowing_empty, position, expr } => {
                let mut sb = self.expr(expr)?;
                let sa = sb.atom();
                let name = self.v.new_var_name(&var.name);
                let pos = match position {
                    Some(p) => self.v.new_var_name(&p.name),
                    None => self.v.new_name(),
                };
                let names = ir::ContextNames { item: name.clone(), position: pos.clone(), last: self.v.new_name() };
                self.locals.push(var.name.clone());
                if let Some(p) = position {
                    self.locals.push(p.name.clone());
                }
                let body = self.type_check(var, &mut |s| s.clauses(rest, tail));
                let empty_body = if *allowing_empty { Some(self.type_check(var, &mut |s| s.clauses(rest, tail))) } else { None };
                self.locals.pop();
                if position.is_some() {
                    self.locals.pop();
                }
                let m = ir::Expr::Map(ir::Map { context_names: names, var_atom: sa.clone(), return_expr: Box::new(body?.expr()) });
                match empty_body {
                    None => {
                        let m = self.v.new_binding(m, span);
                        Ok(sb.bind(m))
                    }
                    Some(eb) => {
                        // `allowing empty`: an empty binding sequence runs the rest once, with the variable empty and the position 0.
                        let mut ex = self.call_fn("exists", FN_NS, vec![sa], span)?;
                        let xa = ex.atom();
                        let zero = self.int(0, span);
                        let inner = ir::Expr::Let(ir::Let {
                            name: pos,
                            var_expr: Box::new(XSpanned::new(ir::Expr::Atom(zero), span)),
                            return_expr: Box::new(eb?.expr()),
                        });
                        let outer = ir::Expr::Let(ir::Let {
                            name,
                            var_expr: Box::new(self.empty_expr(span)),
                            return_expr: Box::new(XSpanned::new(inner, span)),
                        });
                        let i = ir::Expr::If(ir::If {
                            condition: xa,
                            then: Box::new(XSpanned::new(m, span)),
                            else_: Box::new(XSpanned::new(outer, span)),
                        });
                        let i = self.v.new_binding(i, span);
                        Ok(sb.concat(ex).bind(i))
                    }
                }
            }
            Clause::Let { var, expr } => {
                let vb = self.expr(expr)?;
                let name = self.v.new_var_name(&var.name);
                self.locals.push(var.name.clone());
                let body = self.type_check(var, &mut |s| s.clauses(rest, tail));
                self.locals.pop();
                let l = ir::Expr::Let(ir::Let { name, var_expr: Box::new(vb.expr()), return_expr: Box::new(body?.expr()) });
                Ok(self.bind(l, span))
            }
            Clause::Where(cond) => {
                let mut cb = self.expr(cond)?;
                let ca = cb.atom();
                let body = self.clauses(rest, tail)?;
                let i = self
                    .v
                    .new_binding(ir::Expr::If(ir::If { condition: ca, then: Box::new(body.expr()), else_: Box::new(self.empty_expr(span)) }), span);
                Ok(cb.bind(i))
            }
            Clause::Window(w) => self.window(w, span, &mut |s| s.clauses(rest, tail)),
            Clause::OrderBy { .. } | Clause::GroupBy(_) | Clause::Count(_) => Err(cerr("XPST0003", "a clause is out of place", &first.span)),
        }
    }

    // ---- windows ----

    fn window(&mut self, w: &Window, span: XSpan, rest: &mut dyn FnMut(&mut Self) -> C<Bindings>) -> C<Bindings> {
        let seq_b = self.expr(&w.expr)?;
        let seq = self.v.new_name();
        let seq_atom = self.var_atom(&seq, span);
        let mut count = self.call_fn("count", FN_NS, vec![seq_atom.clone()], span)?;
        let count_atom = count.atom();
        // start: function($i) { let $s := …, $s-pos := $i, … return boolean(when) }, sharing the context of the clause
        let i = self.v.new_name();
        let start_body = self.condition_vars(&w.start, &seq_atom, &self.var_atom(&i, span), span, &mut |s| {
            let mut wb = s.expr(&w.start.when)?;
            let wa = wb.atom();
            let b = s.call_fn("boolean", FN_NS, vec![wa], span)?;
            Ok(wb.concat(b))
        })?;
        let start_fn = ir::Expr::FunctionDefinition(ir::FunctionDefinition {
            params: vec![ir::Param { name: i, type_: None }],
            return_type: None,
            body: Box::new(start_body.expr()),
        });
        let mut start_b = self.bind(start_fn, span);
        let start_atom = start_b.atom();
        let mut end_b = Bindings::empty();
        let (end_atom, only) = match &w.end {
            Some((only, end)) => {
                let sp = self.v.new_name();
                let j = self.v.new_name();
                let sp_atom = self.var_atom(&sp, span);
                let j_atom = self.var_atom(&j, span);
                let body = self.condition_vars(&w.start, &seq_atom, &sp_atom, span, &mut |s| {
                    s.condition_vars(end, &seq_atom, &j_atom, span, &mut |s2| {
                        let mut wb = s2.expr(&end.when)?;
                        let wa = wb.atom();
                        let b = s2.call_fn("boolean", FN_NS, vec![wa], span)?;
                        Ok(wb.concat(b))
                    })
                })?;
                let end_fn = ir::Expr::FunctionDefinition(ir::FunctionDefinition {
                    params: vec![ir::Param { name: sp, type_: None }, ir::Param { name: j, type_: None }],
                    return_type: None,
                    body: Box::new(body.expr()),
                });
                let mut eb = self.bind(end_fn, span);
                let ea = eb.atom();
                end_b = eb;
                (ea, *only)
            }
            None => (self.konst(ir::Const::EmptySequence, span), false),
        };
        let mut only_b = self.boolean(only, span)?;
        let only_atom = only_b.atom();
        let finder = if w.tumbling { "tumbling" } else { "sliding" };
        let mut windows = self.prelude_call(finder, vec![count_atom, start_atom, end_atom, only_atom], span)?;
        let windows_atom = windows.atom();
        // for each window [start, end]: bind the window and the condition variables, then the rest
        let wa = self.v.new_name();
        let wa_atom = self.var_atom(&wa, span);
        let names = ir::ContextNames { item: wa.clone(), position: self.v.new_name(), last: self.v.new_name() };
        let one = self.int(1, span);
        let two = self.int(2, span);
        let mut s_pos = self.call_atom(wa_atom.clone(), vec![one], span);
        let s_pos_atom = s_pos.atom();
        let mut e_pos = self.call_atom(wa_atom.clone(), vec![two], span);
        let e_pos_atom = e_pos.atom();
        let mut diff =
            self.bind(ir::Expr::Binary(ir::Binary { left: e_pos_atom.clone(), op: xast::BinaryOperator::Sub, right: s_pos_atom.clone() }), span);
        let diff_atom = diff.atom();
        let one2 = self.int(1, span);
        let mut length = self.bind(ir::Expr::Binary(ir::Binary { left: diff_atom, op: xast::BinaryOperator::Add, right: one2 }), span);
        let length_atom = length.atom();
        let window_value = self.call_fn("subsequence", FN_NS, vec![seq_atom.clone(), s_pos_atom.clone(), length_atom], span)?;
        let w_name = self.v.new_var_name(&w.var.name);
        self.locals.push(w.var.name.clone());
        let inner = self.condition_vars(&w.start, &seq_atom, &s_pos_atom, span, &mut |s| match &w.end {
            Some((_, end)) => s.condition_vars(end, &seq_atom, &e_pos_atom, span, &mut |s2| s2.type_check(&w.var, rest)),
            None => s.type_check(&w.var, rest),
        });
        self.locals.pop();
        let body = ir::Expr::Let(ir::Let { name: w_name, var_expr: Box::new(window_value.expr()), return_expr: Box::new(inner?.expr()) });
        let body = s_pos.concat(e_pos).concat(diff).concat(length).bind(self.v.new_binding(body, span));
        let m = self.v.new_binding(ir::Expr::Map(ir::Map { context_names: names, var_atom: windows_atom, return_expr: Box::new(body.expr()) }), span);
        let all = count.concat(start_b).concat(end_b).concat(only_b).concat(windows).bind(m);
        let l = ir::Expr::Let(ir::Let { name: seq, var_expr: Box::new(seq_b.expr()), return_expr: Box::new(all.expr()) });
        Ok(self.bind(l, span))
    }

    /// Bind the variables of a window condition (`$s`, `at $p`, `previous $q`, `next $n`) around `body`.
    fn condition_vars(
        &mut self,
        cond: &WindowCondition,
        seq: &ir::AtomS,
        position: &ir::AtomS,
        span: XSpan,
        body: &mut dyn FnMut(&mut Self) -> C<Bindings>,
    ) -> C<Bindings> {
        let mut lets: Vec<(ir::Name, Bindings)> = Vec::new();
        let one = self.int(1, span);
        if let Some(v) = &cond.current {
            let b = self.call_fn("subsequence", FN_NS, vec![seq.clone(), position.clone(), one.clone()], span)?;
            lets.push((self.v.new_var_name(&v.name), b));
        }
        if let Some(v) = &cond.position {
            let b = self.atom_b(position.clone());
            lets.push((self.v.new_var_name(&v.name), b));
        }
        if let Some(v) = &cond.previous {
            let mut p = self.bind(ir::Expr::Binary(ir::Binary { left: position.clone(), op: xast::BinaryOperator::Sub, right: one.clone() }), span);
            let pa = p.atom();
            let b = self.call_fn("subsequence", FN_NS, vec![seq.clone(), pa, one.clone()], span)?;
            lets.push((self.v.new_var_name(&v.name), p.concat(b)));
        }
        if let Some(v) = &cond.next {
            let mut p = self.bind(ir::Expr::Binary(ir::Binary { left: position.clone(), op: xast::BinaryOperator::Add, right: one.clone() }), span);
            let pa = p.atom();
            let b = self.call_fn("subsequence", FN_NS, vec![seq.clone(), pa, one.clone()], span)?;
            lets.push((self.v.new_var_name(&v.name), p.concat(b)));
        }
        let names: Vec<Name> = [&cond.current, &cond.position, &cond.previous, &cond.next].into_iter().flatten().map(|v| v.name.clone()).collect();
        self.locals.extend(names.iter().cloned());
        let inner = body(self);
        for _ in &names {
            self.locals.pop();
        }
        let mut inner = inner?.expr();
        for (name, value) in lets.into_iter().rev() {
            let s = inner.span;
            inner = XSpanned::new(ir::Expr::Let(ir::Let { name, var_expr: Box::new(value.expr()), return_expr: Box::new(inner) }), s);
        }
        Ok(self.bind(inner.value, inner.span))
    }

    // ---- constructors ----

    fn xml_name(&mut self, name: &Name, span: XSpan) -> Bindings {
        let local = self.string(name.local_name(), span);
        let ns = self.string(name.namespace(), span);
        self.bind(ir::Expr::XmlName(ir::XmlName { local_name: local, namespace: ns }), span)
    }

    /// Give a constructed element the namespace node its prefix needs (unless the element declares it itself), so the prefix
    /// written in the query is the one serialized.
    fn prefix_namespace(&mut self, b: Bindings, name: &Name, declared: &[(String, String)], span: XSpan) -> Bindings {
        let prefix = name.prefix();
        let ns = name.namespace();
        if ns.is_empty() || declared.iter().any(|(p, _)| p == prefix) || prefix == "xml" {
            return b;
        }
        let mut b = b;
        let ea = b.atom();
        let p = self.string(prefix, span);
        let u = self.string(ns, span);
        let mut nsb = self.bind(ir::Expr::XmlNamespace(ir::XmlNamespace { prefix: p, namespace: u }), span);
        let na = nsb.atom();
        let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: ea, child: na }), span);
        b.concat(nsb).bind(append)
    }

    fn direct_element(
        &mut self,
        name: &Name,
        namespaces: &[(String, String)],
        attributes: &[DirAttribute],
        content: &[DirContent],
        span: XSpan,
    ) -> C<Bindings> {
        let mut nb = self.xml_name(name, span);
        let na = nb.atom();
        let el = self.v.new_binding(ir::Expr::XmlElement(ir::XmlElement { name: na }), span);
        let mut b = nb.bind(el);
        let elem = b.atom();
        // (`atom` popped nothing: the element binding is not an atom, so its name stays bound.)
        b = b.bind(self.v.new_binding(ir::Expr::Atom(elem.clone()), span));
        for (prefix, uri) in namespaces {
            let p = self.string(prefix, span);
            let u = self.string(uri, span);
            let mut nsb = self.bind(ir::Expr::XmlNamespace(ir::XmlNamespace { prefix: p, namespace: u }), span);
            let nsa = nsb.atom();
            let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: elem.clone(), child: nsa }), span);
            b = b.concat(nsb).bind(append);
        }
        b = self.prefix_namespace(b, name, namespaces, span);
        self.ns_stack.push(namespaces.to_vec());
        let result = self.direct_element_parts(b, &elem, name, namespaces, attributes, content, span);
        self.ns_stack.pop();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn direct_element_parts(
        &mut self,
        mut b: Bindings,
        elem: &ir::AtomS,
        _name: &Name,
        namespaces: &[(String, String)],
        attributes: &[DirAttribute],
        content: &[DirContent],
        span: XSpan,
    ) -> C<Bindings> {
        for a in attributes {
            let aspan = self.sp(&a.span);
            let mut value = Bindings::empty();
            let mut atoms = Vec::new();
            for p in &a.value {
                match p {
                    AttrPart::Text(t) => atoms.push(self.string(t, aspan)),
                    AttrPart::Expr(e) => {
                        let mut jb = self.joined(e, " ")?;
                        atoms.push(jb.atom());
                        value = value.concat(jb);
                    }
                }
            }
            let mut joined = self.string_join(atoms, "", aspan)?;
            let va = joined.atom();
            let mut anb = self.xml_name(&a.name, aspan);
            let ana = anb.atom();
            let mut attr = self.bind(ir::Expr::XmlAttribute(ir::XmlAttribute { name: ana, value: va }), aspan);
            let aa = attr.atom();
            let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: elem.clone(), child: aa }), aspan);
            let mut chunk = value.concat(joined).concat(anb).concat(attr).bind(append);
            // the attribute's prefix
            let prefix = a.name.prefix();
            if !a.name.namespace().is_empty() && !prefix.is_empty() && prefix != "xml" && !namespaces.iter().any(|(p, _)| p == prefix) {
                let p = self.string(prefix, aspan);
                let u = self.string(a.name.namespace(), aspan);
                let mut nsb = self.bind(ir::Expr::XmlNamespace(ir::XmlNamespace { prefix: p, namespace: u }), aspan);
                let nsa = nsb.atom();
                let ap = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: elem.clone(), child: nsa }), aspan);
                chunk = chunk.concat(nsb).bind(ap);
            }
            b = b.concat(self.seal(chunk));
        }
        // the content, in chunks so that a long one does not nest deeply
        let mut chunk = Bindings::empty();
        let mut in_chunk = 0usize;
        for c in content {
            let child = match c {
                DirContent::Text { text, boundary } => {
                    if *boundary && !self.boundary_preserve {
                        continue;
                    }
                    let t = self.string(text, span);
                    Some(self.bind(ir::Expr::XmlText(ir::XmlText { value: t }), span))
                }
                DirContent::Enclosed(Some(e)) => Some(self.expr(e)?),
                DirContent::Enclosed(None) => None,
                DirContent::Node(e) => Some(self.expr(e)?),
            };
            let Some(mut child) = child else { continue };
            let ca = child.atom();
            let append = self.v.new_binding(ir::Expr::XmlAppend(ir::XmlAppend { parent: elem.clone(), child: ca }), span);
            chunk = chunk.concat(child).bind(append);
            in_chunk += 1;
            if in_chunk >= 16 {
                b = b.concat(self.seal(chunk));
                chunk = Bindings::empty();
                in_chunk = 0;
            }
        }
        if in_chunk > 0 {
            b = b.concat(self.seal(chunk));
        }
        // the value is the element
        Ok(b.bind(self.v.new_binding(ir::Expr::Atom(elem.clone()), span)))
    }

    /// The name of a computed element or attribute: written (a constant), or computed (`xqi:qname` resolves a string against
    /// the prefixes in scope).
    fn comp_name(&mut self, name: &CompName, element: bool, span: XSpan) -> C<Bindings> {
        match name {
            CompName::Name(n) => Ok(self.xml_name(n, span)),
            CompName::NcName(n) => Ok(self.xml_name(&Name::new(n.clone(), String::new(), String::new()), span)),
            CompName::Expr(e) => {
                let mut eb = self.expr(e)?;
                let ea = eb.atom();
                let mut map_b = self.prefix_map(span);
                let ma = map_b.atom();
                let default = if element { self.current_default_element() } else { String::new() };
                let d = self.string(&default, span);
                let kind = self.boolean(element, span)?;
                let mut kind = kind;
                let ka = kind.atom();
                let mut q = self.prelude_call("qname", vec![ea, ma, d, ka], span)?;
                let qa = q.atom();
                let mut local = self.call_fn("local-name-from-QName", FN_NS, vec![qa.clone()], span)?;
                let la = local.atom();
                let mut ns = self.call_fn("namespace-uri-from-QName", FN_NS, vec![qa], span)?;
                let nsa = ns.atom();
                let x = self.v.new_binding(ir::Expr::XmlName(ir::XmlName { local_name: la, namespace: nsa }), span);
                Ok(eb.concat(map_b).concat(kind).concat(q).concat(local).concat(ns).bind(x))
            }
        }
    }

    fn current_default_element(&self) -> String {
        for scope in self.ns_stack.iter().rev() {
            if let Some((_, u)) = scope.iter().find(|(p, _)| p.is_empty()) {
                return u.clone();
            }
        }
        self.default_element.clone()
    }

    /// The prefixes in scope as a map constant (for names computed at run time).
    fn prefix_map(&mut self, span: XSpan) -> Bindings {
        let mut all = self.prefixes.clone();
        for scope in &self.ns_stack {
            for (p, u) in scope {
                if !p.is_empty() {
                    all.insert(p.clone(), u.clone());
                }
            }
        }
        let mut entries: Vec<(String, String)> = all.into_iter().collect();
        entries.sort();
        let members = entries.iter().map(|(p, u)| (self.string(p, span), self.string(u, span))).collect();
        self.bind(ir::Expr::MapConstructor(ir::MapConstructor { members }), span)
    }
}

fn display_name(n: &Name) -> String {
    if !n.prefix().is_empty() {
        format!("{}:{}", n.prefix(), n.local_name())
    } else if n.namespace().is_empty() {
        n.local_name().to_owned()
    } else {
        format!("Q{{{}}}{}", n.namespace(), n.local_name())
    }
}

fn binop(op: BinOp) -> xast::BinaryOperator {
    use xast::BinaryOperator as B;
    match op {
        BinOp::Or => B::Or,
        BinOp::And => B::And,
        BinOp::ValueEq => B::ValueEq,
        BinOp::ValueNe => B::ValueNe,
        BinOp::ValueLt => B::ValueLt,
        BinOp::ValueLe => B::ValueLe,
        BinOp::ValueGt => B::ValueGt,
        BinOp::ValueGe => B::ValueGe,
        BinOp::GenEq => B::GenEq,
        BinOp::GenNe => B::GenNe,
        BinOp::GenLt => B::GenLt,
        BinOp::GenLe => B::GenLe,
        BinOp::GenGt => B::GenGt,
        BinOp::GenGe => B::GenGe,
        BinOp::Is => B::Is,
        BinOp::Precedes => B::Precedes,
        BinOp::Follows => B::Follows,
        BinOp::Concat => B::Concat,
        BinOp::Range => B::Range,
        BinOp::Add => B::Add,
        BinOp::Sub => B::Sub,
        BinOp::Mul => B::Mul,
        BinOp::Div => B::Div,
        BinOp::IntDiv => B::IntDiv,
        BinOp::Mod => B::Mod,
        BinOp::Union => B::Union,
        BinOp::Intersect => B::Intersect,
        BinOp::Except => B::Except,
    }
}

fn cast_type(t: &SingleType, span: &Span) -> C<Xs> {
    let Some(xs) = Xs::by_name(t.name.namespace(), t.name.local_name()) else {
        return Err(cerr("XQST0052", format!("'{}' is not an atomic type", t.name.local_name()), span));
    };
    if !xs.derives_from(Xs::AnySimpleType) {
        return Err(cerr("XQST0052", format!("'{}' is not an atomic type", t.name.local_name()), span));
    }
    if xs == Xs::Notation || xs == Xs::AnySimpleType || xs == Xs::AnyAtomicType {
        return Err(cerr("XPST0080", format!("nothing can be cast to '{}'", t.name.local_name()), span));
    }
    Ok(xs)
}

/// The variables the clauses bind, each once, in order.
fn bound_vars(clauses: &[Spanned<Clause>]) -> Vec<Name> {
    let mut out: Vec<Name> = Vec::new();
    let mut add = |n: &Name| {
        if !out.contains(n) {
            out.push(n.clone());
        }
    };
    for c in clauses {
        match &c.value {
            Clause::For { var, position, .. } => {
                add(&var.name);
                if let Some(p) = position {
                    add(&p.name);
                }
            }
            Clause::Let { var, .. } => add(&var.name),
            Clause::Window(w) => {
                add(&w.var.name);
                let conds = std::iter::once(&w.start).chain(w.end.as_ref().map(|(_, e)| e));
                for cond in conds {
                    for v in [&cond.current, &cond.position, &cond.previous, &cond.next].into_iter().flatten() {
                        add(&v.name);
                    }
                }
            }
            Clause::Count(v) => add(&v.name),
            Clause::GroupBy(specs) => {
                for s in specs {
                    add(&s.var.name);
                }
            }
            Clause::Where(_) | Clause::OrderBy { .. } => {}
        }
    }
    out
}
