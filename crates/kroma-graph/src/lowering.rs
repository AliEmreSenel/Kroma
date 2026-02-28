//! AST → node-graph lowering.
//!
//! Uses the `glsl` crate to parse Shadertoy-style GLSL into a
//! [`glsl::syntax::TranslationUnit`], then walks the AST to produce
//! a [`ShaderGraph`] with properly wired nodes.

use std::collections::HashMap;

use glsl::parser::Parse;
use glsl::syntax::{
    self, BinaryOp, CompoundStatement, Declaration, Expr, ExternalDeclaration, ForInitStatement,
    ForRestStatement, FunIdentifier, FunctionDefinition, Identifier, InitDeclaratorList,
    Initializer, IterationStatement, JumpStatement, SelectionRestStatement, SelectionStatement,
    SimpleStatement, Statement, TranslationUnit, TypeSpecifierNonArray, UnaryOp,
};

use crate::graph::ShaderGraph;
use crate::types::*;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a Shadertoy-compatible GLSL fragment into a [`ShaderGraph`].
///
/// Wraps the source in a minimal scaffolding if it doesn't contain
/// `mainImage` before feeding it to the `glsl` parser.
pub fn parse_glsl_to_graph(src: &str) -> ShaderGraph {
    let prepped = prepare_source(src);

    #[cfg(debug_assertions)]
    eprintln!(
        "[kroma-graph] Parsing GLSL ({} chars, prepared {} chars)",
        src.len(),
        prepped.len()
    );

    let tu = match TranslationUnit::parse(&prepped) {
        Ok(tu) => {
            #[cfg(debug_assertions)]
            eprintln!(
                "[kroma-graph] GLSL parsed OK — {} top-level declarations",
                tu.0.0.len()
            );
            tu
        }
        Err(e) => {
            eprintln!("[kroma-graph] GLSL parse FAILED: {:?}", e);
            return ShaderGraph::new();
        }
    };

    let mut ctx = LowerCtx::new();
    ctx.lower_translation_unit(&tu);

    #[cfg(debug_assertions)]
    {
        let node_count = ctx.graph.nodes().count();
        let conn_count = ctx.graph.connections().len();
        eprintln!(
            "[kroma-graph] Lowered to graph: {} nodes, {} connections",
            node_count, conn_count
        );
    }

    // Auto-layout nodes based on dependency tree
    ctx.graph.auto_layout();

    #[cfg(debug_assertions)]
    for node in ctx.graph.nodes() {
        eprintln!(
            "[kroma-graph]   {:?} → {:?} at ({:.0}, {:.0})",
            node.id, node.kind, node.position[0], node.position[1]
        );
    }

    ctx.graph
}

// ---------------------------------------------------------------------------
// Source preparation
// ---------------------------------------------------------------------------

/// Strip preprocessor lines, layout() declarations, and normalize
/// Kroma-translated shaders back to Shadertoy-compatible GLSL.
fn prepare_source(src: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut inside_uniform_block = false;

    for line in src.lines() {
        let trimmed = line.trim_start();

        // Skip #version and #extension directives (keep #define and other macros)
        if trimmed.starts_with("#version") || trimmed.starts_with("#extension") {
            continue;
        }
        // Skip layout(...) declarations (uniform blocks, sampler bindings, outputs)
        if trimmed.starts_with("layout(") || trimmed.starts_with("layout (") {
            if trimmed.contains("uniform ") && trimmed.contains('{') {
                inside_uniform_block = true;
            }
            continue;
        }
        // Skip Kroma auto-generated comment blocks
        if trimmed.starts_with("// ====") || trimmed.starts_with("// Auto-generated") {
            continue;
        }
        // Skip uniform declarations (and track block opening)
        if trimmed.starts_with("uniform ") {
            if trimmed.contains('{') {
                inside_uniform_block = true;
            }
            continue;
        }
        // Skip everything inside a uniform block until its closing brace
        if inside_uniform_block {
            if trimmed == "};" || trimmed == "}" {
                inside_uniform_block = false;
            }
            continue;
        }

        lines.push(line);
    }

    let mut stripped = lines.join("\n");

    // Normalize kroma_main() back to mainImage()
    if stripped.contains("kroma_main") {
        stripped = stripped.replace(
            "void kroma_main()",
            "void mainImage(out vec4 fragColor, in vec2 fragCoord)",
        );
        // Remove the injected local variable declarations
        // (vec2 fragCoord = ...; vec4 fragColor = ...;)
        stripped = stripped
            .lines()
            .filter(|l| {
                let t = l.trim();
                // Skip injected fragCoord / fragColor locals
                !(t.starts_with("vec2 fragCoord =")
                    || t.starts_with("vec4 fragColor =")
                    || t.starts_with("kroma_out_color ="))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Remove the `void main() { kroma_main(); }` wrapper if still present.
    // Match flexibly regardless of whitespace/indentation.
    {
        let src_lines: Vec<&str> = stripped.lines().collect();
        let mut out_lines: Vec<&str> = Vec::new();
        let mut skip_depth: Option<u32> = None;
        for line in &src_lines {
            let t = line.trim();
            if skip_depth.is_none() {
                // Only match `void main(` — NOT `void mainImage(` which is the actual shader function
                let is_void_main =
                    t.starts_with("void") && t.contains("main(") && !t.contains("mainImage");
                if is_void_main {
                    // Check that the body only calls kroma_main() or mainImage()
                    let body_is_forwarder = t.contains("kroma_main")
                        || (t.contains("mainImage") && !t.contains("void mainImage"));
                    if body_is_forwarder || t.ends_with('{') {
                        skip_depth = Some(0);
                        for ch in t.chars() {
                            match ch {
                                '{' => *skip_depth.as_mut().unwrap() += 1,
                                '}' => {
                                    let d = skip_depth.as_mut().unwrap();
                                    *d = d.saturating_sub(1);
                                    if *d == 0 {
                                        skip_depth = None;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                }
                out_lines.push(line);
            } else {
                for ch in t.chars() {
                    match ch {
                        '{' => *skip_depth.as_mut().unwrap() += 1,
                        '}' => {
                            let d = skip_depth.as_mut().unwrap();
                            *d = d.saturating_sub(1);
                            if *d == 0 {
                                skip_depth = None;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        stripped = out_lines.join("\n");
    }

    // If it already has mainImage, use it directly.
    if stripped.contains("mainImage") {
        return stripped;
    }

    // Wrap bare code in a mainImage function.
    format!("void mainImage(out vec4 fragColor, in vec2 fragCoord) {{\n{stripped}\n}}")
}

// ---------------------------------------------------------------------------
// Lowering context
// ---------------------------------------------------------------------------

struct LowerCtx {
    graph: ShaderGraph,
    /// Maps variable names → (node_id, output_port) of the node that produces
    /// that variable's value.
    vars: HashMap<String, PortAddr>,
    /// Tracks GLSL-level variable widths from declarations (vec2→2, vec3→3, etc.)
    /// to avoid incorrect width inference from node output types.
    var_widths: HashMap<String, usize>,
    /// Layout column counter for auto-placement.
    col: f32,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            graph: ShaderGraph::new(),
            vars: HashMap::new(),
            var_widths: HashMap::new(),
            col: 0.0,
        }
    }

    /// Place a new node at the current column and advance.
    fn add_node(&mut self, kind: NodeKind) -> NodeId {
        let pos = [self.col * 260.0, 100.0];
        self.col += 1.0;
        self.graph.add_node(kind, pos)
    }

    fn connect(&mut self, from: PortAddr, to: PortAddr) {
        self.graph.force_add_connection(from, to);
    }

    // -- Translation unit --------------------------------------------------

    fn lower_translation_unit(&mut self, tu: &TranslationUnit) {
        for decl in &tu.0 {
            match decl {
                ExternalDeclaration::FunctionDefinition(fd) => {
                    self.lower_function_def(fd);
                }
                ExternalDeclaration::Declaration(_) => {
                    // Uniforms, globals — skip (Shadertoy provides them).
                }
                _ => {}
            }
        }
    }

    fn lower_function_def(&mut self, fd: &FunctionDefinition) {
        let name = fd.prototype.name.0.as_str();
        if name == "mainImage" || name == "kroma_main" {
            self.lower_compound(&fd.statement);
        } else {
            // Store helper function text for codegen.
            // We do NOT create a CustomFunc node here — function definitions
            // are stored verbatim for codegen. Call sites create CustomFunc
            // nodes with proper argument connections.
            let mut buf = String::new();
            glsl::transpiler::glsl::show_function_definition(&mut buf, fd);
            if !buf.is_empty() {
                self.graph.helper_functions.push((name.to_string(), buf));
            }
        }
    }

    fn lower_compound(&mut self, cs: &CompoundStatement) {
        for stmt in &cs.statement_list {
            self.lower_statement(stmt);
        }
    }

    // -- Statements --------------------------------------------------------

    fn lower_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Simple(s) => self.lower_simple(s),
            Statement::Compound(cs) => self.lower_compound(cs),
        }
    }

    fn lower_simple(&mut self, s: &SimpleStatement) {
        match s {
            SimpleStatement::Declaration(d) => self.lower_declaration(d),
            SimpleStatement::Expression(Some(e)) => {
                self.lower_expr(e);
            }
            SimpleStatement::Selection(sel) => self.lower_selection(sel),
            SimpleStatement::Iteration(iter) => self.lower_iteration(iter),
            SimpleStatement::Jump(j) => self.lower_jump(j),
            _ => {}
        }
    }

    // -- Declarations (variable initializations) ---------------------------

    fn lower_declaration(&mut self, d: &Declaration) {
        if let Declaration::InitDeclaratorList(idl) = d {
            self.lower_init_declarator_list(idl);
        }
    }

    fn lower_init_declarator_list(&mut self, idl: &InitDeclaratorList) {
        // Extract type width from declaration type specifier
        let decl_width = Self::type_spec_width(&idl.head.ty.ty.ty);

        let sd = &idl.head;
        if let Some(name) = &sd.name {
            let var_name = name.0.clone();
            // Track GLSL-level width from the declaration type
            self.var_widths.insert(var_name.clone(), decl_width);
            if let Some(Initializer::Simple(init_expr)) = &sd.initializer {
                let src = self.lower_expr(init_expr);
                self.vars.insert(var_name.clone(), src);
                // Preserve original variable name for roundtrip
                if let Some(node) = self.graph.node_mut(src.node)
                    && node.label.is_none() {
                        node.label = Some(var_name.clone());
                    }
            }
        }
        // Tail declarations (multiple in one line)
        for tail in &idl.tail {
            let var_name = tail.ident.ident.0.clone();
            if let Some(Initializer::Simple(init_expr)) = &tail.initializer {
                let src = self.lower_expr(init_expr);
                self.vars.insert(var_name.clone(), src);
                // Preserve original variable name for roundtrip
                if let Some(node) = self.graph.node_mut(src.node)
                    && node.label.is_none() {
                        node.label = Some(var_name.clone());
                    }
            }
        }
    }

    // -- Selection (if/else → Conditional node) ----------------------------

    fn lower_selection(&mut self, sel: &SelectionStatement) {
        let cond_src = self.lower_expr(&sel.cond);
        let node_id = self.add_node(NodeKind::Conditional);

        // Connect condition → port 0 ("Cond")
        self.connect(
            cond_src,
            PortAddr {
                node: node_id,
                port: 0,
            },
        );
        // Set Thresh (port 1) to 0.5 as default (condition > 0.5 → true)
        if let Some(node) = self.graph.node_mut(node_id)
            && node.defaults.len() > 1 {
                node.defaults[1] = DefaultValue::Float(0.5);
            }

        match &sel.rest {
            SelectionRestStatement::Statement(then_stmt) => {
                // Lower the entire branch body for side effects (variable assignments,
                // function calls, etc.), then extract the final value for the node wire.
                self.lower_statement(then_stmt);
                if let Some(val) = self.extract_branch_value(then_stmt) {
                    self.connect(
                        val,
                        PortAddr {
                            node: node_id,
                            port: 2,
                        },
                    ); // "True"
                }
            }
            SelectionRestStatement::Else(then_stmt, else_stmt) => {
                // Lower both branches for side effects first
                self.lower_statement(then_stmt);
                if let Some(val) = self.extract_branch_value(then_stmt) {
                    self.connect(
                        val,
                        PortAddr {
                            node: node_id,
                            port: 2,
                        },
                    ); // "True"
                }
                self.lower_statement(else_stmt);
                if let Some(val) = self.extract_branch_value(else_stmt) {
                    self.connect(
                        val,
                        PortAddr {
                            node: node_id,
                            port: 3,
                        },
                    ); // "False"
                }
            }
        }

        // If the branch assigns to a known variable, update the var map.
        // Track which variable was assigned inside branches.
        if let Some(var_name) = self.extract_branch_assigned_var(&sel.rest) {
            self.vars.insert(
                var_name,
                PortAddr {
                    node: node_id,
                    port: 0,
                },
            );
        }
    }

    fn extract_branch_value(&self, stmt: &Statement) -> Option<PortAddr> {
        // After lowering the branch body, look up the value of the last
        // assignment target from the vars map instead of re-lowering.
        match stmt {
            Statement::Simple(boxed) => match boxed.as_ref() {
                SimpleStatement::Expression(Some(Expr::Assignment(lhs, _, _rhs))) => {
                    // The lhs was assigned during lowering — look it up in vars
                    if let Expr::Variable(ident) = lhs.as_ref() {
                        self.vars.get(ident.0.as_str()).copied()
                    } else {
                        None
                    }
                }
                _ => None,
            },
            Statement::Compound(cs) => {
                // Take the last statement's value
                cs.statement_list
                    .last()
                    .and_then(|s| self.extract_branch_value(s))
            }
        }
    }

    // -- Iteration (for/while → ForLoop node) ------------------------------

    fn lower_iteration(&mut self, iter: &IterationStatement) {
        match iter {
            IterationStatement::For(init, rest, body) => {
                self.lower_for_loop(init, rest, body);
            }
            IterationStatement::While(cond, body) => {
                // Map while → ForLoop with a high iteration count
                let node_id = self.add_node(NodeKind::ForLoop);
                if let syntax::Condition::Expr(cond_expr) = cond {
                    let src = self.lower_expr(cond_expr);
                    self.connect(
                        src,
                        PortAddr {
                            node: node_id,
                            port: 0,
                        },
                    ); // init
                }
                self.lower_statement(body);
                if let Some(val) = self.extract_branch_value(body) {
                    self.connect(
                        val,
                        PortAddr {
                            node: node_id,
                            port: 2,
                        },
                    ); // body
                }
            }
            IterationStatement::DoWhile(body, _cond) => {
                // Same strategy
                let node_id = self.add_node(NodeKind::ForLoop);
                self.lower_statement(body);
                if let Some(val) = self.extract_branch_value(body) {
                    self.connect(
                        val,
                        PortAddr {
                            node: node_id,
                            port: 2,
                        },
                    );
                }
            }
        }
    }

    fn lower_for_loop(
        &mut self,
        init: &ForInitStatement,
        rest: &ForRestStatement,
        body: &Statement,
    ) {
        let node_id = self.add_node(NodeKind::ForLoop);

        // Init: try to extract the initial value
        match init {
            ForInitStatement::Declaration(boxed_decl) => {
                if let Declaration::InitDeclaratorList(idl) = boxed_decl.as_ref()
                    && let Some(Initializer::Simple(init_expr)) = &idl.head.initializer {
                        let src = self.lower_expr(init_expr);
                        self.connect(
                            src,
                            PortAddr {
                                node: node_id,
                                port: 0,
                            },
                        ); // "Init"

                        // Register the loop var so the body can reference it
                        if let Some(name) = &idl.head.name {
                            self.vars.insert(
                                name.0.clone(),
                                PortAddr {
                                    node: node_id,
                                    port: 0, // output port 0 of ForLoop = result
                                },
                            );
                        }
                    }
            }
            ForInitStatement::Expression(Some(e)) => {
                let src = self.lower_expr(e);
                self.connect(
                    src,
                    PortAddr {
                        node: node_id,
                        port: 0,
                    },
                );
            }
            _ => {}
        }

        // Condition: try to extract the upper bound as "Count"
        if let Some(syntax::Condition::Expr(cond_expr)) = &rest.condition
            && let Some(count_src) = self.extract_loop_bound(cond_expr) {
                self.connect(
                    count_src,
                    PortAddr {
                        node: node_id,
                        port: 1,
                    },
                ); // "Count"
            }

        // Body: lower all statements for side effects, then extract value
        match body {
            Statement::Compound(cs) => {
                self.lower_compound(cs);
            }
            Statement::Simple(s) => {
                self.lower_simple(s);
            }
        }
        // Try to get the last value from the body for connecting to port 2
        if let Some(val) = self.extract_branch_value(body) {
            self.connect(
                val,
                PortAddr {
                    node: node_id,
                    port: 2,
                },
            ); // "Body"
        }
    }

    /// Try to extract the upper bound from `i < N` or `i <= N`.
    fn extract_loop_bound(&mut self, cond: &Expr) -> Option<PortAddr> {
        match cond {
            Expr::Binary(BinaryOp::LT | BinaryOp::LTE, _lhs, rhs) => Some(self.lower_expr(rhs)),
            Expr::Binary(BinaryOp::GT | BinaryOp::GTE, lhs, _rhs) => Some(self.lower_expr(lhs)),
            _ => None,
        }
    }

    // -- Jump (used for detecting fragColor assignment) --------------------

    fn lower_jump(&mut self, j: &JumpStatement) {
        // `return expr;` in mainImage is rare but handle it
        if let JumpStatement::Return(Some(expr)) = j {
            let src = self.lower_expr(expr);
            // Connect to Output node (id 1)
            let output_id = NodeId(1);
            self.connect(
                src,
                PortAddr {
                    node: output_id,
                    port: 0,
                },
            );
        }
    }

    // -- Expression lowering (the core) ------------------------------------

    /// Lower an expression, returning the PortAddr of the output that
    /// produces its value.
    fn lower_expr(&mut self, expr: &Expr) -> PortAddr {
        match expr {
            // -- Literals ---
            Expr::FloatConst(f) => self.make_float_const(*f as f64),
            Expr::DoubleConst(d) => self.make_float_const(*d),
            Expr::IntConst(i) => self.make_float_const(*i as f64),
            Expr::UIntConst(u) => self.make_float_const(*u as f64),
            Expr::BoolConst(b) => self.make_float_const(if *b { 1.0 } else { 0.0 }),

            // -- Variable reference ---
            Expr::Variable(ident) => self.lower_variable(ident),

            // -- Binary operator ---
            Expr::Binary(op, lhs, rhs) => self.lower_binary(op.clone(), lhs, rhs),

            // -- Unary operator ---
            Expr::Unary(op, operand) => self.lower_unary(op.clone(), operand),

            // -- Ternary ---
            Expr::Ternary(cond, t, f) => self.lower_ternary(cond, t, f),

            // -- Function call / type constructor ---
            Expr::FunCall(fun_id, args) => self.lower_fun_call(fun_id, args),

            // -- Dot (member/swizzle) ---
            Expr::Dot(object, field) => self.lower_dot(object, field),

            // -- Assignment (as expression) ---
            Expr::Assignment(lhs, op, rhs) => {
                let rhs_src = self.lower_expr(rhs);

                // Handle compound assignment: +=, -=, *=, /=
                let final_src = match op {
                    syntax::AssignmentOp::Equal => rhs_src,
                    compound_op => {
                        // Get the current value of the LHS variable
                        let lhs_val = self.lower_expr(lhs);
                        let kind = match compound_op {
                            syntax::AssignmentOp::Add => NodeKind::Add,
                            syntax::AssignmentOp::Sub => NodeKind::Subtract,
                            syntax::AssignmentOp::Mult => NodeKind::Multiply,
                            syntax::AssignmentOp::Div => NodeKind::Divide,
                            syntax::AssignmentOp::Mod => NodeKind::Mod,
                            _ => NodeKind::Add, // fallback
                        };
                        let node_id = self.add_node(kind);
                        self.connect(
                            lhs_val,
                            PortAddr {
                                node: node_id,
                                port: 0,
                            },
                        );
                        self.connect(
                            rhs_src,
                            PortAddr {
                                node: node_id,
                                port: 1,
                            },
                        );
                        PortAddr {
                            node: node_id,
                            port: 0,
                        }
                    }
                };

                // Check if LHS is a swizzle write (e.g., color.rgb = ...)
                if let Expr::Dot(base_expr, field) = lhs.as_ref()
                    && is_swizzle(&field.0) {
                        if let Expr::Variable(ident) = base_expr.as_ref() {
                            // Update the variable with the swizzle write result
                            self.vars.insert(ident.0.clone(), final_src);
                            if is_frag_color(&ident.0) {
                                self.connect(
                                    final_src,
                                    PortAddr {
                                        node: NodeId(1),
                                        port: 0,
                                    },
                                );
                            }
                        }
                        return final_src;
                    }

                // Update var map if lhs is a variable
                if let Expr::Variable(ident) = lhs.as_ref() {
                    self.vars.insert(ident.0.clone(), final_src);

                    // If this is fragColor, connect to Output
                    if is_frag_color(&ident.0) {
                        self.connect(
                            final_src,
                            PortAddr {
                                node: NodeId(1),
                                port: 0,
                            },
                        );
                    }
                }
                final_src
            }

            // -- PostInc / PostDec (treat as identity for graph) ---
            Expr::PostInc(e) | Expr::PostDec(e) => self.lower_expr(e),

            // -- Comma ---
            Expr::Comma(_a, b) => self.lower_expr(b),

            // -- Bracket (array access — fallback to FloatConst) ---
            _ => self.make_glsl_fallback(expr),
        }
    }

    // -- Variable resolution -----------------------------------------------

    fn lower_variable(&mut self, ident: &Identifier) -> PortAddr {
        let name = &ident.0;

        // Already-computed variable
        if let Some(&addr) = self.vars.get(name.as_str()) {
            return addr;
        }

        // Shadertoy uniforms
        match name.as_str() {
            "iTime" | "u_time" => {
                let id = self.add_node(NodeKind::Time);
                PortAddr { node: id, port: 0 }
            }
            "iTimeDelta" => {
                let id = self.add_node(NodeKind::DeltaTime);
                PortAddr { node: id, port: 0 }
            }
            "iFrame" => {
                let id = self.add_node(NodeKind::Frame);
                PortAddr { node: id, port: 0 }
            }
            "iResolution" | "u_resolution" => {
                let id = self.add_node(NodeKind::Resolution);
                PortAddr { node: id, port: 0 }
            }
            "iMouse" => {
                let id = self.add_node(NodeKind::Mouse);
                PortAddr { node: id, port: 0 }
            }
            "fragCoord" | "gl_FragCoord" => {
                let id = self.add_node(NodeKind::UV);
                PortAddr { node: id, port: 0 }
            }
            _ => {
                // Unknown variable — treat as an external constant / already-declared name
                let node_id = self.add_node(NodeKind::FloatConst);
                if let Some(node) = self.graph.node_mut(node_id) {
                    node.label = Some(name.clone());
                    node.meta = Some(name.clone());
                }
                self.vars.insert(
                    name.clone(),
                    PortAddr {
                        node: node_id,
                        port: 0,
                    },
                );
                PortAddr {
                    node: node_id,
                    port: 0,
                }
            }
        }
    }

    // -- Binary ops --------------------------------------------------------

    fn lower_binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> PortAddr {
        let kind = match op {
            BinaryOp::Add => NodeKind::Add,
            BinaryOp::Sub => NodeKind::Subtract,
            BinaryOp::Mult => NodeKind::Multiply,
            BinaryOp::Div => NodeKind::Divide,
            BinaryOp::Mod => NodeKind::Mod,
            BinaryOp::LT => NodeKind::LessThan,
            BinaryOp::GT => NodeKind::GreaterThan,
            BinaryOp::LTE => NodeKind::LessEqual,
            BinaryOp::GTE => NodeKind::GreaterEqual,
            BinaryOp::Equal => NodeKind::Equal,
            BinaryOp::NonEqual => NodeKind::NotEqual,
            BinaryOp::And => NodeKind::LogicalAnd,
            BinaryOp::Or => NodeKind::LogicalOr,
            BinaryOp::BitAnd => NodeKind::BitAnd,
            BinaryOp::BitOr => NodeKind::BitOr,
            BinaryOp::BitXor => NodeKind::BitXor,
            BinaryOp::LShift => NodeKind::LeftShift,
            BinaryOp::RShift => NodeKind::RightShift,
            _ => return self.make_glsl_fallback_binop(op, lhs, rhs),
        };

        let node_id = self.add_node(kind);
        self.lower_arg_or_default(lhs, node_id, 0);
        self.lower_arg_or_default(rhs, node_id, 1);
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    fn make_glsl_fallback_binop(&mut self, _op: BinaryOp, lhs: &Expr, rhs: &Expr) -> PortAddr {
        // Truly unknown binary ops — create a FloatConst fallback
        let _l = self.lower_expr(lhs);
        let _r = self.lower_expr(rhs);
        let node_id = self.add_node(NodeKind::FloatConst);
        if let Some(node) = self.graph.node_mut(node_id) {
            node.meta = Some("[unknown binop]".to_string());
        }
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    // -- Unary ops ---------------------------------------------------------

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expr) -> PortAddr {
        // Optimization: -literal → FloatConst(-literal) instead of Negate node
        if matches!(op, UnaryOp::Minus)
            && let Some(val) = Self::try_as_float_literal(operand) {
                return self.make_float_const((-val) as f64);
            }

        let kind = match op {
            UnaryOp::Minus => NodeKind::Negate,
            UnaryOp::Add => return self.lower_expr(operand), // +x = x
            UnaryOp::Not => NodeKind::LogicalNot,
            UnaryOp::Complement => NodeKind::BitNot,
            UnaryOp::Inc | UnaryOp::Dec => return self.lower_expr(operand),
        };

        let node_id = self.add_node(kind);
        self.lower_arg_or_default(operand, node_id, 0);
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    // -- Ternary -----------------------------------------------------------

    fn lower_ternary(&mut self, cond: &Expr, t: &Expr, f: &Expr) -> PortAddr {
        let node_id = self.add_node(NodeKind::Conditional);
        // Port 0 = Cond, Port 1 = Thresh (default 0.5), Port 2 = True, Port 3 = False
        self.lower_arg_or_default(cond, node_id, 0);
        if let Some(node) = self.graph.node_mut(node_id)
            && node.defaults.len() > 1 {
                node.defaults[1] = DefaultValue::Float(0.5);
            }
        self.lower_arg_or_default(t, node_id, 2);
        self.lower_arg_or_default(f, node_id, 3);
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    // -- Function calls / type constructors --------------------------------

    fn lower_fun_call(&mut self, fun_id: &FunIdentifier, args: &[Expr]) -> PortAddr {
        let name = match fun_id {
            FunIdentifier::Identifier(ident) => ident.0.as_str(),
            FunIdentifier::Expr(e) => {
                // Rare: function pointer or computed call — fall through
                // to CustomFunc path which processes arguments
                let node_id = self.add_node(NodeKind::CustomFunc);
                // Lower the function expression itself as source for meta
                let fn_src = self.lower_expr(e);
                self.connect(
                    fn_src,
                    PortAddr {
                        node: node_id,
                        port: 0,
                    },
                );
                for (i, arg) in args.iter().enumerate().take(7) {
                    let src = self.lower_expr(arg);
                    self.connect(
                        src,
                        PortAddr {
                            node: node_id,
                            port: i + 1,
                        },
                    );
                }
                return PortAddr {
                    node: node_id,
                    port: 0,
                };
            }
        };

        // Type constructors: vec2, vec3, vec4, float, int
        match name {
            "vec2" => return self.lower_combine(2, args),
            "vec3" => return self.lower_combine(3, args),
            "vec4" => return self.lower_combine(4, args),
            "float" | "int" | "uint" | "double" => {
                if args.len() == 1 {
                    return self.lower_expr(&args[0]);
                }
            }
            _ => {}
        }

        // Known single-arg math functions
        if args.len() == 1 {
            let kind = match name {
                "sin" => Some(NodeKind::Sin),
                "cos" => Some(NodeKind::Cos),
                "tan" => Some(NodeKind::Tan),
                "asin" => Some(NodeKind::Asin),
                "acos" => Some(NodeKind::Acos),
                "atan" => Some(NodeKind::Atan),
                "exp" => Some(NodeKind::Exp),
                "exp2" => Some(NodeKind::Exp2),
                "log" => Some(NodeKind::Log),
                "log2" => Some(NodeKind::Log2),
                "sqrt" => Some(NodeKind::Sqrt),
                "inversesqrt" => Some(NodeKind::InverseSqrt),
                "abs" => Some(NodeKind::Abs),
                "sign" => Some(NodeKind::Sign),
                "floor" => Some(NodeKind::Floor),
                "ceil" => Some(NodeKind::Ceil),
                "fract" => Some(NodeKind::Fract),
                "round" => Some(NodeKind::Round),
                "normalize" => Some(NodeKind::Normalize),
                "length" => Some(NodeKind::Length),
                _ => None,
            };
            if let Some(k) = kind {
                let node_id = self.add_node(k);
                self.lower_arg_or_default(&args[0], node_id, 0);
                return PortAddr {
                    node: node_id,
                    port: 0,
                };
            }
        }

        // Known two-arg math functions
        if args.len() == 2 {
            let kind = match name {
                "pow" => Some(NodeKind::Power),
                "mod" => Some(NodeKind::Mod),
                "min" => Some(NodeKind::Min),
                "max" => Some(NodeKind::Max),
                "step" => Some(NodeKind::Step),
                "dot" => Some(NodeKind::Dot),
                "distance" => Some(NodeKind::Distance),
                "reflect" => Some(NodeKind::Reflect),
                "atan" => Some(NodeKind::Atan2),
                "cross" => Some(NodeKind::Cross),
                _ => None,
            };
            if let Some(k) = kind {
                let node_id = self.add_node(k);
                self.lower_arg_or_default(&args[0], node_id, 0);
                self.lower_arg_or_default(&args[1], node_id, 1);
                return PortAddr {
                    node: node_id,
                    port: 0,
                };
            }
        }

        // Known three-arg functions
        if args.len() == 3 {
            let kind = match name {
                "clamp" => Some(NodeKind::Clamp),
                "mix" | "lerp" => Some(NodeKind::Mix),
                "smoothstep" => Some(NodeKind::SmoothStep),
                "refract" => Some(NodeKind::Refract),
                _ => None,
            };
            if let Some(k) = kind {
                let node_id = self.add_node(k);
                self.lower_arg_or_default(&args[0], node_id, 0);
                self.lower_arg_or_default(&args[1], node_id, 1);
                self.lower_arg_or_default(&args[2], node_id, 2);
                return PortAddr {
                    node: node_id,
                    port: 0,
                };
            }
        }

        // Texture sampling: port 0 = Channel (Float), port 1 = UV (Vec2)
        if name == "texture" || name.starts_with("texture2D") {
            let node_id = self.add_node(NodeKind::TextureSample);
            // Extract channel index from sampler name (iChannel0 → 0, iChannel1 → 1, etc.)
            if let Some(Expr::Variable(ident)) = args.first()
                && let Some(ch) = ident
                    .0
                    .strip_prefix("iChannel")
                    .and_then(|s| s.parse::<f32>().ok())
                    && let Some(node) = self.graph.node_mut(node_id) {
                        node.defaults[0] = DefaultValue::Float(ch);
                    }
            // Connect UV to port 1
            if args.len() >= 2 {
                let uv = self.lower_expr(&args[1]);
                self.connect(
                    uv,
                    PortAddr {
                        node: node_id,
                        port: 1,
                    },
                );
            }
            return PortAddr {
                node: node_id,
                port: 0,
            };
        }

        // Matrix constructors — use dedicated Mat2/Mat3/Mat4 nodes
        if matches!(name, "mat2" | "mat3" | "mat4") {
            let mat_kind = match name {
                "mat2" => NodeKind::Mat2,
                "mat3" => NodeKind::Mat3,
                "mat4" => NodeKind::Mat4,
                _ => unreachable!(),
            };
            let node_id = self.add_node(mat_kind);
            // Store the full constructor expression in meta for codegen
            let lowered_args: Vec<PortAddr> = args.iter().map(|arg| self.lower_expr(arg)).collect();
            let arg_vars: Vec<String> = lowered_args
                .iter()
                .map(|pa| format!("n{}_{}", pa.node, pa.port))
                .collect();
            if let Some(node) = self.graph.node_mut(node_id) {
                node.meta = Some(format!("{}({})", name, arg_vars.join(", ")));
            }
            for (i, src) in lowered_args.into_iter().enumerate().take(4) {
                self.connect(
                    src,
                    PortAddr {
                        node: node_id,
                        port: i,
                    },
                );
            }
            return PortAddr {
                node: node_id,
                port: 0,
            };
        }

        // Fallback: CustomFunc node — stores function name and arg count
        let node_id = self.add_node(NodeKind::CustomFunc);
        let arg_count = args.len().min(8); // CustomFunc supports up to 8 inputs
        if let Some(node) = self.graph.node_mut(node_id) {
            node.meta = Some(format!("{}:{}", name, arg_count));
        }
        for (i, arg) in args.iter().enumerate().take(8) {
            let src = self.lower_expr(arg);
            self.connect(
                src,
                PortAddr {
                    node: node_id,
                    port: i,
                },
            );
        }
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    // -- Vector constructors -----------------------------------------------

    fn lower_combine(&mut self, components: usize, args: &[Expr]) -> PortAddr {
        // Single-arg broadcast: vec3(x) connects source to all ports
        if args.len() == 1 {
            let kind = match components {
                2 => NodeKind::Combine2,
                3 => NodeKind::Combine3,
                4 => NodeKind::Combine4,
                _ => return self.lower_expr(&args[0]),
            };
            let node_id = self.add_node(kind);
            let src = self.lower_expr(&args[0]);
            for i in 0..components {
                self.connect(
                    src,
                    PortAddr {
                        node: node_id,
                        port: i,
                    },
                );
            }
            return PortAddr {
                node: node_id,
                port: 0,
            };
        }

        let kind = match components {
            2 => NodeKind::Combine2,
            3 => NodeKind::Combine3,
            4 => NodeKind::Combine4,
            _ => return self.make_float_const(0.0),
        };

        let node_id = self.add_node(kind);

        // Handle mixed-size args: vec4(vec2, float, float), vec4(vec3, float), etc.
        // Track actual component width consumed by each argument.
        let mut port_idx = 0;
        for arg in args {
            if port_idx >= components {
                break;
            }
            // Estimate the width of this argument
            let arg_width = self.estimate_expr_width(arg);
            if arg_width > 1 && port_idx + arg_width <= components {
                // Multi-component argument: split it and connect individual components
                let src = self.lower_expr(arg);
                let split_kind = match arg_width {
                    2 => NodeKind::SplitVec2,
                    3 => NodeKind::SplitVec3,
                    _ => NodeKind::SplitVec4,
                };
                let split_id = self.add_node(split_kind);
                self.connect(
                    src,
                    PortAddr {
                        node: split_id,
                        port: 0,
                    },
                );
                for comp in 0..arg_width {
                    if port_idx < components {
                        self.connect(
                            PortAddr {
                                node: split_id,
                                port: comp,
                            },
                            PortAddr {
                                node: node_id,
                                port: port_idx,
                            },
                        );
                        port_idx += 1;
                    }
                }
            } else {
                self.lower_arg_or_default(arg, node_id, port_idx);
                port_idx += 1;
            }
        }

        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    // -- Dot / swizzle -----------------------------------------------------

    fn lower_dot(&mut self, object: &Expr, field: &Identifier) -> PortAddr {
        let field_str = &field.0;

        // iResolution.xy → UV pattern
        if let Expr::Variable(ident) = object
            && is_resolution(&ident.0)
                && (field_str == "xy" || field_str == "x" || field_str == "y")
            {
                let id = self.add_node(NodeKind::Resolution);
                return PortAddr { node: id, port: 0 };
            }

        let obj_src = self.lower_expr(object);

        // Swizzle: .x, .y, .z, .w, .r, .g, .b, .a, .xy, .xyz, etc.
        if is_swizzle(field_str) {
            let n_components = field_str.len();
            if n_components == 1 {
                // Single component → SplitVec sized to match input width
                let width = self.estimate_expr_width(object);
                let component_port = match field_str.chars().next() {
                    Some('x' | 'r' | 's') => 0,
                    Some('y' | 'g' | 't') => 1,
                    Some('z' | 'b' | 'p') => 2,
                    Some('w' | 'a' | 'q') => 3,
                    _ => 0,
                };
                // If the requested component is beyond the input's width,
                // return 0.0 (e.g. iMouse.z when Mouse outputs vec2)
                if component_port >= width {
                    return self.make_float_const(0.0);
                }
                let split_kind = match width {
                    2 => NodeKind::SplitVec2,
                    3 => NodeKind::SplitVec3,
                    _ => NodeKind::SplitVec4,
                };
                let node_id = self.add_node(split_kind);
                self.connect(
                    obj_src,
                    PortAddr {
                        node: node_id,
                        port: 0,
                    },
                );
                return PortAddr {
                    node: node_id,
                    port: component_port,
                };
            }
            // Multi-component swizzle → Split then Combine (sized to input)
            let width = self.estimate_expr_width(object);
            let split_kind = match width {
                2 => NodeKind::SplitVec2,
                3 => NodeKind::SplitVec3,
                _ => NodeKind::SplitVec4,
            };
            let split_id = self.add_node(split_kind);
            self.connect(
                obj_src,
                PortAddr {
                    node: split_id,
                    port: 0,
                },
            );

            let combine_kind = match n_components {
                2 => NodeKind::Combine2,
                3 => NodeKind::Combine3,
                _ => NodeKind::Combine4,
            };
            let combine_id = self.add_node(combine_kind);
            for (i, ch) in field_str.chars().enumerate() {
                let split_port = match ch {
                    'x' | 'r' | 's' => 0,
                    'y' | 'g' | 't' => 1,
                    'z' | 'b' | 'p' => 2,
                    'w' | 'a' | 'q' => 3,
                    _ => 0,
                };
                // If split_port is beyond the input width, use 0.0 constant
                let src = if split_port >= width {
                    self.make_float_const(0.0)
                } else {
                    PortAddr {
                        node: split_id,
                        port: split_port,
                    }
                };
                self.connect(
                    src,
                    PortAddr {
                        node: combine_id,
                        port: i,
                    },
                );
            }
            return PortAddr {
                node: combine_id,
                port: 0,
            };
        }

        // Non-swizzle member access — just pass through
        obj_src
    }

    // -- Helpers -----------------------------------------------------------

    /// Extract component width from a GLSL type specifier.
    fn type_spec_width(ts: &TypeSpecifierNonArray) -> usize {
        match ts {
            TypeSpecifierNonArray::Vec2
            | TypeSpecifierNonArray::IVec2
            | TypeSpecifierNonArray::BVec2 => 2,
            TypeSpecifierNonArray::Vec3
            | TypeSpecifierNonArray::IVec3
            | TypeSpecifierNonArray::BVec3 => 3,
            TypeSpecifierNonArray::Vec4
            | TypeSpecifierNonArray::IVec4
            | TypeSpecifierNonArray::BVec4 => 4,
            TypeSpecifierNonArray::Mat2 => 4,
            TypeSpecifierNonArray::Mat3 => 9,
            TypeSpecifierNonArray::Mat4 => 16,
            _ => 1,
        }
    }

    /// If `expr` is a numeric literal, return the value as f32.
    fn try_as_float_literal(expr: &Expr) -> Option<f32> {
        match expr {
            Expr::FloatConst(f) => Some(*f),
            Expr::DoubleConst(d) => Some(*d as f32),
            Expr::IntConst(i) => Some(*i as f32),
            Expr::UIntConst(u) => Some(*u as f32),
            Expr::BoolConst(b) => Some(if *b { 1.0 } else { 0.0 }),
            Expr::Unary(UnaryOp::Minus, inner) => Self::try_as_float_literal(inner).map(|v| -v),
            _ => None,
        }
    }

    /// Lower an argument destined for `target_node` at `target_port`.
    /// If the argument is a literal float, set it as the inline default
    /// instead of creating a FloatConst node + wire.
    /// Lower an argument and explicitly connect it to the target port.
    /// Always creates a connection - literal values produce FloatConst nodes.
    fn lower_arg_or_default(&mut self, arg: &Expr, target_node: NodeId, target_port: usize) {
        let src = self.lower_expr(arg);
        self.connect(
            src,
            PortAddr {
                node: target_node,
                port: target_port,
            },
        );
    }

    fn make_float_const(&mut self, val: f64) -> PortAddr {
        let node_id = self.add_node(NodeKind::FloatConst);
        // Set the default value
        if let Some(node) = self.graph.node_mut(node_id)
            && !node.defaults.is_empty() {
                node.defaults[0] = DefaultValue::Float(val as f32);
            }
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    fn make_glsl_fallback(&mut self, _expr: &Expr) -> PortAddr {
        let node_id = self.add_node(NodeKind::FloatConst);
        if let Some(node) = self.graph.node_mut(node_id) {
            node.meta = Some("[complex expr]".to_string());
        }
        PortAddr {
            node: node_id,
            port: 0,
        }
    }

    /// Extract the variable name assigned in a conditional branch.
    fn extract_branch_assigned_var(&self, rest: &SelectionRestStatement) -> Option<String> {
        match rest {
            SelectionRestStatement::Statement(stmt) => self.extract_assigned_var_from_stmt(stmt),
            SelectionRestStatement::Else(then_stmt, _else_stmt) => {
                self.extract_assigned_var_from_stmt(then_stmt)
            }
        }
    }

    fn extract_assigned_var_from_stmt(&self, stmt: &Statement) -> Option<String> {
        match stmt {
            Statement::Simple(boxed) => match boxed.as_ref() {
                SimpleStatement::Expression(Some(Expr::Assignment(lhs, _, _))) => {
                    if let Expr::Variable(ident) = lhs.as_ref() {
                        Some(ident.0.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            },
            Statement::Compound(cs) => cs
                .statement_list
                .last()
                .and_then(|s| self.extract_assigned_var_from_stmt(s)),
        }
    }

    /// Estimate the vector width of an expression (1=float, 2=vec2, 3=vec3, 4=vec4).
    fn estimate_expr_width(&self, expr: &Expr) -> usize {
        match expr {
            Expr::FloatConst(_)
            | Expr::DoubleConst(_)
            | Expr::IntConst(_)
            | Expr::UIntConst(_)
            | Expr::BoolConst(_) => 1,
            Expr::Variable(ident) => {
                // Check if it's a known variable with a known width
                if let Some(addr) = self.vars.get(ident.0.as_str())
                    && let Some(node) = self.graph.node(addr.node) {
                        let outputs = node.outputs();
                        if let Some(out) = outputs.get(addr.port) {
                            return match out.data_type {
                                DataType::Float => 1,
                                DataType::Vec2 => 2,
                                DataType::Vec3 => 3,
                                DataType::Vec4 => 4,
                            };
                        }
                    }
                match ident.0.as_str() {
                    "iResolution" => 3,
                    "u_resolution" | "fragCoord" | "gl_FragCoord" => 2,
                    "iMouse" => 4,
                    _ => 1,
                }
            }
            Expr::FunCall(FunIdentifier::Identifier(ident), args) => {
                match ident.0.as_str() {
                    "vec2" => 2,
                    "vec3" => 3,
                    "vec4" => 4,
                    "float" | "int" | "uint" | "sin" | "cos" | "tan" | "abs" | "floor" | "ceil"
                    | "fract" | "sqrt" | "length" | "dot" | "distance" | "step" | "sign"
                    | "mod" | "pow" | "min" | "max" => 1,
                    "normalize" | "cross" | "reflect" | "refract" => {
                        // These return the same dimension as their first argument
                        if !args.is_empty() {
                            self.estimate_expr_width(&args[0])
                        } else {
                            3
                        }
                    }
                    "mix" | "clamp" | "smoothstep" => {
                        if !args.is_empty() {
                            self.estimate_expr_width(&args[0])
                        } else {
                            1
                        }
                    }
                    _ => 1,
                }
            }
            Expr::Dot(_, field) => {
                let field_str = &field.0;
                if is_swizzle(field_str) {
                    field_str.len()
                } else {
                    1
                }
            }
            Expr::Binary(_op, lhs, rhs) => {
                // Arithmetic on vectors preserves the wider operand's width
                let lw = self.estimate_expr_width(lhs);
                let rw = self.estimate_expr_width(rhs);
                lw.max(rw)
            }
            _ => 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn is_frag_color(name: &str) -> bool {
    matches!(name, "fragColor" | "gl_FragColor" | "kroma_out_color")
}

fn is_resolution(name: &str) -> bool {
    matches!(name, "iResolution" | "u_resolution")
}

fn is_swizzle(field: &str) -> bool {
    !field.is_empty()
        && field.chars().all(|c| {
            matches!(
                c,
                'x' | 'y' | 'z' | 'w' | 'r' | 'g' | 'b' | 'a' | 's' | 't' | 'p' | 'q'
            )
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn count_nodes_of_kind(graph: &ShaderGraph, kind: &NodeKind) -> usize {
        graph.nodes().filter(|n| &n.kind == kind).count()
    }

    #[test]
    fn test_simple_color() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                fragColor = vec4(1.0, 0.0, 0.0, 1.0);
            }",
        );
        // Should have Output + Combine4 + 4 FloatConsts
        assert!(count_nodes_of_kind(&graph, &NodeKind::Combine4) >= 1);
        assert!(graph.connections().len() > 0);
    }

    #[test]
    fn test_uv_calculation() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                fragColor = vec4(uv, 0.0, 1.0);
            }",
        );
        // Should have UV or Resolution nodes
        let has_resolution = count_nodes_of_kind(&graph, &NodeKind::Resolution) > 0
            || count_nodes_of_kind(&graph, &NodeKind::UV) > 0;
        assert!(has_resolution);
    }

    #[test]
    fn test_sin_time() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                float t = sin(iTime);
                fragColor = vec4(t, t, t, 1.0);
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::Sin) >= 1);
        assert!(count_nodes_of_kind(&graph, &NodeKind::Time) >= 1);
    }

    #[test]
    fn test_for_loop() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec4 col = vec4(0.0);
                for (int i = 0; i < 10; i++) {
                    col += vec4(1.0);
                }
                fragColor = col;
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::ForLoop) >= 1);
    }

    #[test]
    fn test_ternary() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                fragColor = uv.x > 0.5 ? vec4(1.0) : vec4(0.0);
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::Conditional) >= 1);
    }

    #[test]
    fn test_if_else() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                if (uv.x > 0.5) {
                    fragColor = vec4(1.0, 0.0, 0.0, 1.0);
                } else {
                    fragColor = vec4(0.0, 0.0, 1.0, 1.0);
                }
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::Conditional) >= 1);
    }

    #[test]
    fn test_math_functions() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                float a = abs(sin(iTime));
                float b = pow(a, 2.0);
                float c = clamp(b, 0.0, 1.0);
                fragColor = vec4(c, c, c, 1.0);
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::Abs) >= 1);
        assert!(count_nodes_of_kind(&graph, &NodeKind::Sin) >= 1);
        assert!(count_nodes_of_kind(&graph, &NodeKind::Power) >= 1);
        assert!(count_nodes_of_kind(&graph, &NodeKind::Clamp) >= 1);
    }

    #[test]
    fn test_complex_shadertoy() {
        let src = r#"
            void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                float t = iTime;
                vec3 col = vec3(0.0);
                for (int i = 0; i < 5; i++) {
                    float fi = float(i);
                    col += 0.5 + 0.5 * cos(t + uv.x * 6.2831 + vec3(0.0, 2.0, 4.0));
                }
                col /= 5.0;
                fragColor = vec4(col, 1.0);
            }
        "#;
        let graph = parse_glsl_to_graph(src);
        // Should produce a non-trivial graph
        assert!(graph.nodes().count() > 5);
        assert!(graph.connections().len() > 3);
    }

    #[test]
    fn test_empty_returns_default() {
        let graph = parse_glsl_to_graph("");
        // Should at least have the Output node
        assert!(graph.nodes().count() >= 1);
    }

    #[test]
    fn test_texture_sample() {
        let graph = parse_glsl_to_graph(
            "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
                vec2 uv = fragCoord / iResolution.xy;
                fragColor = texture(iChannel0, uv);
            }",
        );
        assert!(count_nodes_of_kind(&graph, &NodeKind::TextureSample) >= 1);
    }

    #[test]
    fn test_kroma_translated_shader() {
        // Simulates the format produced by the Kroma translator
        let src = r#"
#version 450

// ============================
// Auto-generated by Kroma
// ============================

layout(set = 0, binding = 0) uniform Globals {
    vec2 u_resolution;
    float u_time;
    float u_time_delta;
    float u_frame;
    vec4 u_mouse;
    vec4 u_date;
};

layout(set = 1, binding = 0) uniform texture2D t_channel0;
layout(set = 1, binding = 1) uniform sampler s_channel0;

layout(location = 0) out vec4 kroma_out_color;

void kroma_main() {
    vec2 fragCoord = gl_FragCoord.xy;
    vec4 fragColor = vec4(0.0);
    vec2 uv = fragCoord / u_resolution.xy;
    fragColor = vec4(uv, 0.5 + 0.5 * sin(u_time), 1.0);
    kroma_out_color = fragColor;
}

void main() {
    kroma_main();
}
"#;
        let graph = parse_glsl_to_graph(src);
        // Should produce nodes for the UV calculation, sin, combine, etc.
        assert!(
            graph.nodes().count() > 3,
            "Expected >3 nodes, got {}",
            graph.nodes().count()
        );
    }

    #[test]
    fn test_prepare_source_strips_kroma_format() {
        let src = r#"
#version 450

layout(set = 0, binding = 0) uniform Globals {
    float u_time;
};

layout(location = 0) out vec4 kroma_out_color;

void kroma_main() {
    vec2 fragCoord = gl_FragCoord.xy;
    vec4 fragColor = vec4(0.0);
    fragColor = vec4(1.0, 0.0, 0.0, 1.0);
    kroma_out_color = fragColor;
}

void main() {
    kroma_main();
}
"#;
        let prepared = prepare_source(src);
        assert!(
            !prepared.contains("layout("),
            "layout() should be stripped: {prepared}"
        );
        assert!(
            !prepared.contains("#version"),
            "#version should be stripped: {prepared}"
        );
        assert!(
            !prepared.contains("kroma_main"),
            "kroma_main should be renamed to mainImage: {prepared}"
        );
        assert!(
            prepared.contains("mainImage"),
            "Should contain mainImage: {prepared}"
        );
        assert!(
            !prepared.contains("kroma_out_color"),
            "kroma_out_color should be stripped: {prepared}"
        );
    }

    #[test]
    fn roundtrip_preserves_variable_names() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float t = iTime;
    vec3 col = vec3(uv.x, uv.y, t);
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        let glsl = graph.compile_glsl().unwrap();
        // Original variable names should be preserved
        assert!(
            glsl.contains("uv") || glsl.contains("n0"),
            "Expected variable name 'uv' or 'n0' in output: {}",
            glsl
        );
        assert!(glsl.contains("mainImage"), "Missing mainImage: {}", glsl);
        assert!(glsl.contains("fragColor"), "Missing fragColor: {}", glsl);
    }

    #[test]
    fn roundtrip_basic_math() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float brightness = 0.5 + 0.5 * sin(iTime);
    fragColor = vec4(vec3(brightness), 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        let glsl = graph.compile_glsl().unwrap();
        assert!(glsl.contains("mainImage"));
        assert!(glsl.contains("sin"));
        // Should compile back without errors
        assert!(glsl.contains("fragColor"));
    }

    #[test]
    fn roundtrip_helper_function_preserved() {
        let src = r#"
float circle(vec2 uv, vec2 center, float radius) {
    return smoothstep(radius, radius - 0.01, length(uv - center));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float c = circle(uv, vec2(0.5), 0.3);
    fragColor = vec4(vec3(c), 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        let glsl = graph.compile_glsl().unwrap();
        // Helper function should be preserved verbatim
        assert!(
            glsl.contains("circle"),
            "Helper function 'circle' not preserved: {}",
            glsl
        );
        assert!(
            glsl.contains("smoothstep"),
            "smoothstep not in output: {}",
            glsl
        );
    }

    // ===================================================================
    // Complex roundtrip tests using real shader fragments
    // ===================================================================

    /// Verify parse + compile roundtrip produces valid GLSL with expected
    /// elements. We don't require byte-exact identity — the graph is a lossy
    /// intermediate representation — but key semantic markers must survive.
    fn assert_roundtrip(label: &str, src: &str, must_contain: &[&str]) {
        let graph = parse_glsl_to_graph(src);
        let node_count = graph.nodes().count();
        assert!(node_count > 0, "[{label}] parse produced 0 nodes");
        match graph.compile_glsl() {
            Ok(glsl) => {
                assert!(
                    glsl.contains("mainImage"),
                    "[{label}] output missing mainImage:\n{glsl}"
                );
                assert!(
                    glsl.contains("fragColor"),
                    "[{label}] output missing fragColor:\n{glsl}"
                );
                for marker in must_contain {
                    assert!(
                        glsl.contains(marker),
                        "[{label}] output missing '{marker}':\n{glsl}"
                    );
                }
            }
            Err(e) => {
                panic!("[{label}] compile_glsl failed: {e}");
            }
        }
    }

    #[test]
    fn roundtrip_sun_grid_shader() {
        // Simplified excerpt from a.glsl — helper functions + complex mainImage
        let src = r#"
float sun(vec2 uv, float battery) {
    float val = smoothstep(0.3, 0.29, length(uv));
    float bloom = smoothstep(0.7, 0.0, length(uv));
    float cut = 3.0 * sin((uv.y + iTime * 0.2 * (battery + 0.02)) * 100.0)
                + clamp(uv.y * 14.0 + 1.0, -6.0, 6.0);
    cut = clamp(cut, 0.0, 1.0);
    return clamp(val * cut, 0.0, 1.0) + bloom * 0.6;
}

float grid(vec2 uv, float battery) {
    vec2 size = vec2(uv.y, uv.y * uv.y * 0.2) * 0.01;
    uv += vec2(0.0, iTime * 4.0 * (battery + 0.05));
    uv = abs(fract(uv) - 0.5);
    vec2 lines = smoothstep(size, vec2(0.0), uv);
    lines += smoothstep(size * 5.0, vec2(0.0), uv) * 0.4 * battery;
    return clamp(lines.x + lines.y, 0.0, 3.0);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord.xy - iResolution.xy) / iResolution.y;
    float battery = 1.0;
    float fog = smoothstep(0.1, -0.02, abs(uv.y + 0.2));
    vec3 col = vec3(0.0, 0.1, 0.2);
    col += fog * fog * fog;
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("sun_grid", src, &["sun", "grid", "smoothstep", "mainImage"]);
    }

    #[test]
    fn roundtrip_sdf_helpers() {
        // SDF helper functions from a.glsl
        let src = r#"
float dot2(in vec2 v) { return dot(v, v); }

float sdTrapezoid(in vec2 p, in float r1, float r2, float he) {
    vec2 k1 = vec2(r2, he);
    vec2 k2 = vec2(r2 - r1, 2.0 * he);
    p.x = abs(p.x);
    vec2 ca = vec2(p.x - min(p.x, (p.y < 0.0) ? r1 : r2), abs(p.y) - he);
    vec2 cb = p - k1 + k2 * clamp(dot(k1 - p, k2) / dot2(k2), 0.0, 1.0);
    float s = (cb.x < 0.0 && ca.y < 0.0) ? -1.0 : 1.0;
    return s * sqrt(min(dot2(ca), dot2(cb)));
}

float sdLine(in vec2 p, in vec2 a, in vec2 b) {
    vec2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

float sdBox(in vec2 p, in vec2 b) {
    vec2 d = abs(p) - b;
    return length(max(d, vec2(0))) + min(max(d.x, d.y), 0.0);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float d = sdBox(uv - vec2(0.5), vec2(0.2));
    fragColor = vec4(vec3(d), 1.0);
}
"#;
        assert_roundtrip(
            "sdf_helpers",
            src,
            &["dot2", "sdTrapezoid", "sdLine", "sdBox", "clamp", "dot"],
        );
    }

    #[test]
    fn roundtrip_catmull_rom_spline() {
        // Adapted from b.glsl — struct + array access + catmull-rom
        let src = r#"
float distanceToLineSeg(vec2 p, vec2 a, vec2 b) {
    vec2 ap = p - a;
    vec2 ab = b - a;
    vec2 e = a + clamp(dot(ap, ab) / dot(ab, ab), 0.0, 1.0) * ab;
    return length(p - e);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord.xy / iResolution.xy;
    float fTime = iTime * 0.15;
    float d = distanceToLineSeg(uv, vec2(0.1, 0.2), vec2(0.8, 0.7));
    vec3 c = vec3(d * 7.0 + smoothstep(0.20, 0.3, abs(fract(d * 20.0) - 0.5)));
    c = mix(vec3(0.0, 0.8, 0.9), c, smoothstep(-0.005, 0.0035, d));
    fragColor = vec4(c, 1.0);
}
"#;
        assert_roundtrip(
            "catmull_rom",
            src,
            &["distanceToLineSeg", "smoothstep", "mix", "fract"],
        );
    }

    #[test]
    fn roundtrip_matrix_rotation() {
        // Adapted from d.glsl — matrix constructor functions
        let src = r#"
float sdHexPrism(vec3 p, vec2 h) {
    vec3 q = abs(p);
    return max(q.z - h.y, max((q.x * 0.866025 + q.y * 0.5), q.y) - h.x);
}

float udBox(vec3 p, vec3 b) {
    return length(max(abs(p) - b, 0.0));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord.xy / iResolution.xy;
    uv -= vec2(0.5);
    uv.y /= iResolution.x / iResolution.y;
    vec3 rp = vec3(0.0, 0.0, 1.0);
    vec3 rd = normalize(vec3(uv, 0.3));
    float d = sdHexPrism(rd, vec2(0.3, 0.1));
    fragColor = vec4(vec3(d), 1.0);
}
"#;
        assert_roundtrip(
            "matrix_rotation",
            src,
            &["sdHexPrism", "udBox", "normalize"],
        );
    }

    #[test]
    fn roundtrip_multiple_mix_chains() {
        // Tests deep mix chain — common pattern in Shadertoy shaders
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = vec3(0.0);
    float t = iTime;

    col = mix(col, vec3(1.0, 0.0, 0.0), smoothstep(0.0, 0.3, uv.x));
    col = mix(col, vec3(0.0, 1.0, 0.0), smoothstep(0.3, 0.6, uv.x));
    col = mix(col, vec3(0.0, 0.0, 1.0), smoothstep(0.6, 1.0, uv.x));
    col = mix(col, vec3(1.0), 0.5 + 0.5 * sin(t));

    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("mix_chains", src, &["mix", "smoothstep", "sin"]);
    }

    #[test]
    fn roundtrip_for_loop_accumulation() {
        // A for loop that accumulates color — common raymarching pattern
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = vec3(0.0);
    for (int i = 0; i < 10; i++) {
        float fi = float(i) / 10.0;
        col += vec3(fi) * 0.1;
    }
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        assert!(
            graph.nodes().count() > 3,
            "for_loop_accum: expected >3 nodes, got {}",
            graph.nodes().count()
        );
        // Verify ForLoop node was created
        assert!(
            graph.nodes().any(|n| n.kind == NodeKind::ForLoop),
            "for_loop_accum: expected at least one ForLoop node"
        );
        // Compilation should succeed (cycle-tolerant for ForLoop nodes)
        let glsl = graph
            .compile_glsl()
            .expect("for_loop_accum: compile failed");
        assert!(
            glsl.contains("mainImage"),
            "for_loop_accum: missing mainImage"
        );
        assert!(
            glsl.contains("fragColor"),
            "for_loop_accum: missing fragColor"
        );
    }

    #[test]
    fn roundtrip_if_else_branches() {
        // Complex branching
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col;
    if (uv.x > 0.5) {
        if (uv.y > 0.5) {
            col = vec3(1.0, 0.0, 0.0);
        } else {
            col = vec3(0.0, 1.0, 0.0);
        }
    } else {
        col = vec3(0.0, 0.0, 1.0);
    }
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        assert!(
            graph.nodes().count() > 0,
            "nested_if_else: produced 0 nodes"
        );
        let result = graph.compile_glsl();
        assert!(
            result.is_ok(),
            "nested_if_else: compile failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn roundtrip_texture_sampling() {
        // Texture sampling with coordinate manipulation
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 distorted = uv + 0.02 * sin(uv.y * 30.0 + iTime);
    vec4 tex = texture(iChannel0, distorted);
    vec4 tex2 = texture(iChannel1, uv);
    fragColor = mix(tex, tex2, 0.5);
}
"#;
        assert_roundtrip("texture_sampling", src, &["iChannel", "mix"]);
    }

    #[test]
    fn roundtrip_shadertoy_plasma() {
        // Classic Shadertoy plasma shader
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy * 2.0 - 1.0;
    uv.x *= iResolution.x / iResolution.y;
    float t = iTime;

    float v1 = sin(uv.x * 10.0 + t);
    float v2 = sin(10.0 * (uv.x * sin(t / 2.0) + uv.y * cos(t / 3.0)) + t);
    float cx = uv.x + 0.5 * sin(t / 5.0);
    float cy = uv.y + 0.5 * cos(t / 3.0);
    float v3 = sin(sqrt(100.0 * (cx * cx + cy * cy) + 1.0) + t);

    float v = v1 + v2 + v3;
    vec3 col = vec3(
        sin(v * 3.14159) * 0.5 + 0.5,
        sin(v * 3.14159 + 2.094) * 0.5 + 0.5,
        sin(v * 3.14159 + 4.189) * 0.5 + 0.5
    );
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("plasma", src, &["sin", "cos", "sqrt"]);
    }

    #[test]
    fn roundtrip_audio_visualizer() {
        // Tests Kroma-specific uniforms
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float audio = u_audio_level;
    float cpu = u_cpu;
    float ram = u_ram;
    float bat = u_battery;
    float t = iTime;

    vec3 col = vec3(0.0);
    col.r = uv.x * audio;
    col.g = uv.y * cpu;
    col.b = (1.0 - uv.x) * ram;
    col *= 1.0 + 0.3 * sin(t * 2.0);
    col = clamp(col, 0.0, 1.0);
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("audio_viz", src, &["sin", "clamp"]);
    }

    #[test]
    fn roundtrip_mouse_interaction() {
        // Mouse-based interaction
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 mouse = iMouse.xy / iResolution.xy;
    float d = distance(uv, mouse);
    float glow = 0.02 / d;
    glow = clamp(glow, 0.0, 1.0);
    vec3 col = vec3(glow) * vec3(0.3, 0.6, 1.0);
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("mouse_interaction", src, &["distance", "clamp"]);
    }

    #[test]
    fn roundtrip_multi_function_composition() {
        // Multiple helper functions calling each other
        let src = r#"
float hash(float p) {
    return fract(sin(p) * 43758.5453);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash(i.x + i.y * 57.0);
    float b = hash(i.x + 1.0 + i.y * 57.0);
    float c = hash(i.x + (i.y + 1.0) * 57.0);
    float d = hash(i.x + 1.0 + (i.y + 1.0) * 57.0);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float val = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 5; i++) {
        val += amp * noise(p);
        p *= 2.0;
        amp *= 0.5;
    }
    return val;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float n = fbm(uv * 10.0 + iTime * 0.5);
    vec3 col = mix(vec3(0.1, 0.2, 0.4), vec3(0.9, 0.6, 0.2), n);
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("multi_function", src, &["hash", "noise", "fbm", "mix"]);
    }

    #[test]
    fn roundtrip_kroma_translated_format() {
        // Test the full Kroma-translated shader format end-to-end
        let src = r#"
#version 450

// ============================
// Auto-generated by Kroma
// ============================

layout(set = 0, binding = 0) uniform Globals {
    vec2 u_resolution;
    float u_time;
    float u_time_delta;
    float u_frame;
    vec4 u_mouse;
    vec4 u_date;
};

layout(set = 1, binding = 0) uniform texture2D t_channel0;
layout(set = 1, binding = 1) uniform sampler s_channel0;

layout(location = 0) out vec4 kroma_out_color;

float sdf_circle(vec2 p, vec2 c, float r) {
    return length(p - c) - r;
}

void kroma_main() {
    vec2 fragCoord = gl_FragCoord.xy;
    vec4 fragColor = vec4(0.0);
    vec2 uv = fragCoord / u_resolution.xy;
    float d = sdf_circle(uv, vec2(0.5), 0.2 + 0.1 * sin(u_time));
    float edge = smoothstep(0.01, 0.0, abs(d));
    fragColor = vec4(vec3(edge), 1.0);
    kroma_out_color = fragColor;
}

void main() {
    kroma_main();
}
"#;
        assert_roundtrip("kroma_format", src, &["sdf_circle", "smoothstep", "sin"]);
    }

    #[test]
    fn roundtrip_swizzle_heavy() {
        // Tests that swizzle operations are handled
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = vec3(uv.x, uv.y, 0.5);
    col = col.zyx;
    float r = col.r;
    float g = col.g;
    vec2 rg = col.rg;
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        assert!(graph.nodes().count() > 0, "swizzle_heavy: produced 0 nodes");
        let result = graph.compile_glsl();
        assert!(
            result.is_ok(),
            "swizzle_heavy: compile failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn roundtrip_complex_expressions() {
        // Deeply nested expressions
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float t = iTime;
    float x = sin(cos(tan(uv.x * 3.14159 + t)));
    float y = abs(fract(uv.y * 5.0 + t) - 0.5) * 2.0;
    float z = pow(max(dot(vec2(x, y), vec2(0.5, 0.5)), 0.0), 2.2);
    vec3 col = vec3(x, y, z);
    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip("complex_expressions", src, &["sin", "cos"]);
    }

    #[test]
    fn roundtrip_assignment_operators() {
        // Tests += -= *= operators
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = vec3(0.0);
    col += vec3(uv.x);
    col -= vec3(0.1);
    col *= 2.0;
    col = clamp(col, 0.0, 1.0);
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        assert!(
            graph.nodes().count() > 0,
            "assignment_ops: produced 0 nodes"
        );
        let result = graph.compile_glsl();
        assert!(
            result.is_ok(),
            "assignment_ops: compile failed: {:?}",
            result.err()
        );
    }

    #[test]
    fn roundtrip_full_retrowave_shader() {
        // Full a.glsl retrowave shader — this is the most complex test
        let src = r#"
float sun(vec2 uv, float battery) {
    float val = smoothstep(0.3, 0.29, length(uv));
    float bloom = smoothstep(0.7, 0.0, length(uv));
    float cut = 3.0 * sin((uv.y + iTime * 0.2 * (battery + 0.02)) * 100.0)
                + clamp(uv.y * 14.0 + 1.0, -6.0, 6.0);
    cut = clamp(cut, 0.0, 1.0);
    return clamp(val * cut, 0.0, 1.0) + bloom * 0.6;
}

float grid(vec2 uv, float battery) {
    vec2 size = vec2(uv.y, uv.y * uv.y * 0.2) * 0.01;
    uv += vec2(0.0, iTime * 4.0 * (battery + 0.05));
    uv = abs(fract(uv) - 0.5);
    vec2 lines = smoothstep(size, vec2(0.0), uv);
    lines += smoothstep(size * 5.0, vec2(0.0), uv) * 0.4 * battery;
    return clamp(lines.x + lines.y, 0.0, 3.0);
}

float dot2(in vec2 v) { return dot(v, v); }

float sdTrapezoid(in vec2 p, in float r1, float r2, float he) {
    vec2 k1 = vec2(r2, he);
    vec2 k2 = vec2(r2 - r1, 2.0 * he);
    p.x = abs(p.x);
    vec2 ca = vec2(p.x - min(p.x, (p.y < 0.0) ? r1 : r2), abs(p.y) - he);
    vec2 cb = p - k1 + k2 * clamp(dot(k1 - p, k2) / dot2(k2), 0.0, 1.0);
    float s = (cb.x < 0.0 && ca.y < 0.0) ? -1.0 : 1.0;
    return s * sqrt(min(dot2(ca), dot2(cb)));
}

float sdLine(in vec2 p, in vec2 a, in vec2 b) {
    vec2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

float sdBox(in vec2 p, in vec2 b) {
    vec2 d = abs(p) - b;
    return length(max(d, vec2(0))) + min(max(d.x, d.y), 0.0);
}

float opSmoothUnion(float d1, float d2, float k) {
    float h = clamp(0.5 + 0.5 * (d2 - d1) / k, 0.0, 1.0);
    return mix(d2, d1, h) - k * h * (1.0 - h);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord.xy - iResolution.xy) / iResolution.y;
    float battery = 1.0;
    float fog = smoothstep(0.1, -0.02, abs(uv.y + 0.2));
    vec3 col = vec3(0.0, 0.1, 0.2);

    vec2 sunUV = uv + vec2(0.75, 0.2);
    col = vec3(1.0, 0.2, 1.0);
    float sunVal = sun(sunUV, battery);
    col = mix(col, vec3(1.0, 0.4, 0.1), sunUV.y * 2.0 + 0.2);
    col = mix(vec3(0.0, 0.0, 0.0), col, sunVal);

    col += fog * fog * fog;
    col = mix(vec3(col.r, col.r, col.r) * 0.5, col, battery * 0.7);

    fragColor = vec4(col, 1.0);
}
"#;
        assert_roundtrip(
            "full_retrowave",
            src,
            &[
                "sun",
                "grid",
                "dot2",
                "sdTrapezoid",
                "sdLine",
                "sdBox",
                "opSmoothUnion",
                "smoothstep",
                "mix",
                "clamp",
            ],
        );
    }

    #[test]
    fn roundtrip_empty_mainimage() {
        // Edge case: minimal mainImage
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(0.0, 0.0, 0.0, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        let glsl = graph.compile_glsl().unwrap();
        assert!(glsl.contains("mainImage"), "empty_main: missing mainImage");
        assert!(glsl.contains("fragColor"), "empty_main: missing fragColor");
    }

    #[test]
    fn roundtrip_double_compile() {
        // Parse → compile → parse again → compile again.
        // The second output should still be valid GLSL.
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float t = iTime;
    vec3 col = vec3(uv, 0.5 + 0.5 * sin(t));
    fragColor = vec4(col, 1.0);
}
"#;
        let graph1 = parse_glsl_to_graph(src);
        let glsl1 = graph1.compile_glsl().expect("first compile failed");

        let graph2 = parse_glsl_to_graph(&glsl1);
        let glsl2 = graph2.compile_glsl().expect("second compile failed");

        assert!(
            glsl2.contains("mainImage"),
            "double_compile: second pass missing mainImage"
        );
        assert!(
            glsl2.contains("fragColor"),
            "double_compile: second pass missing fragColor"
        );
        assert!(
            glsl2.contains("sin"),
            "double_compile: second pass missing sin"
        );
    }

    // ===================================================================
    // Structural validation — verify exact node graph for complex shaders
    // ===================================================================

    /// Count nodes of a specific kind in a graph.
    fn count_kind(graph: &ShaderGraph, kind: NodeKind) -> usize {
        graph.nodes().filter(|n| n.kind == kind).count()
    }

    // =====================================================================
    // Structural tests — exact counts, full connection validation
    // =====================================================================

    /// Helper: count nodes by kind
    fn assert_count(graph: &ShaderGraph, kind: NodeKind, expected: usize, shader: &str) {
        let actual = count_kind(graph, kind.clone());
        assert_eq!(
            actual,
            expected,
            "{shader}: expected {expected} {k:?}, got {actual}",
            k = kind
        );
    }

    /// Helper: validate every connection in the graph
    fn validate_connections(graph: &ShaderGraph, shader: &str) {
        for conn in graph.connections() {
            let from_node = graph.node(conn.from.node);
            let to_node = graph.node(conn.to.node);
            assert!(
                from_node.is_some(),
                "{shader}: conn {id:?} invalid source {src:?}",
                id = conn.id,
                src = conn.from.node
            );
            assert!(
                to_node.is_some(),
                "{shader}: conn {id:?} invalid target {dst:?}",
                id = conn.id,
                dst = conn.to.node
            );
            let from_n = from_node.unwrap();
            let to_n = to_node.unwrap();
            assert!(
                conn.from.port < from_n.outputs().len(),
                "{shader}: conn {id:?} src port {p} >= outputs {o} on {k:?}",
                id = conn.id,
                p = conn.from.port,
                o = from_n.outputs().len(),
                k = from_n.kind
            );
            assert!(
                conn.to.port < to_n.inputs().len(),
                "{shader}: conn {id:?} dst port {p} >= inputs {i} on {k:?}",
                id = conn.id,
                p = conn.to.port,
                i = to_n.inputs().len(),
                k = to_n.kind
            );
        }
    }

    #[test]
    fn structural_a_glsl_full_retrowave() {
        let src = include_str!("../../../a.glsl");
        let graph = parse_glsl_to_graph(src);

        // No GlslExpr nodes
        assert_count(&graph, NodeKind::GlslExpr, 0, "a.glsl");

        // Helper functions preserved
        let helper_names: Vec<&str> = graph
            .helper_functions
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        for f in &[
            "sun",
            "grid",
            "dot2",
            "sdTrapezoid",
            "sdLine",
            "sdBox",
            "opSmoothUnion",
            "sdCloud",
        ] {
            assert!(
                helper_names.contains(f),
                "a.glsl missing helper \'{}\'. Found: {:?}",
                f,
                helper_names
            );
        }

        // Exact node counts
        assert_count(&graph, NodeKind::Output, 1, "a.glsl");
        assert_count(&graph, NodeKind::UV, 1, "a.glsl");
        assert_count(&graph, NodeKind::Resolution, 2, "a.glsl");
        assert_count(&graph, NodeKind::Time, 3, "a.glsl");
        assert_count(&graph, NodeKind::Mix, 11, "a.glsl");
        assert_count(&graph, NodeKind::SmoothStep, 5, "a.glsl");
        assert_count(&graph, NodeKind::Multiply, 35, "a.glsl");
        assert_count(&graph, NodeKind::Add, 33, "a.glsl");
        assert_count(&graph, NodeKind::Subtract, 13, "a.glsl");
        assert_count(&graph, NodeKind::Divide, 2, "a.glsl");
        assert_count(&graph, NodeKind::Abs, 4, "a.glsl");
        assert_count(&graph, NodeKind::Step, 3, "a.glsl");
        assert_count(&graph, NodeKind::Sin, 4, "a.glsl");
        assert_count(&graph, NodeKind::Cos, 5, "a.glsl");
        assert_count(&graph, NodeKind::Combine3, 14, "a.glsl");
        assert_count(&graph, NodeKind::Combine2, 11, "a.glsl");
        assert_count(&graph, NodeKind::Combine4, 1, "a.glsl");
        assert_count(&graph, NodeKind::CustomFunc, 5, "a.glsl");
        assert_count(&graph, NodeKind::Conditional, 1, "a.glsl");
        assert_count(&graph, NodeKind::FloatConst, 146, "a.glsl");
        assert_count(&graph, NodeKind::LessThan, 1, "a.glsl");
        assert_count(&graph, NodeKind::Min, 2, "a.glsl");
        assert_count(&graph, NodeKind::Power, 1, "a.glsl");
        assert_count(&graph, NodeKind::Mod, 1, "a.glsl");
        assert_count(&graph, NodeKind::SplitVec2, 1, "a.glsl");
        assert_count(&graph, NodeKind::SplitVec4, 7, "a.glsl");

        // Total counts
        let nodes = graph.nodes().count();
        let conns = graph.connections().len();
        assert_eq!(nodes, 314, "a.glsl total nodes");
        assert_eq!(conns, 346, "a.glsl total connections");

        // Validate all connections
        validate_connections(&graph, "a.glsl");

        // Compiles back to GLSL
        let glsl = graph.compile_glsl().expect("a.glsl failed to compile");
        assert!(
            glsl.contains("mainImage"),
            "a.glsl output missing mainImage"
        );
        assert!(
            glsl.contains("fragColor"),
            "a.glsl output missing fragColor"
        );
        assert!(glsl.contains("float sun("), "a.glsl output missing sun()");
        assert!(glsl.contains("float grid("), "a.glsl output missing grid()");
        assert!(glsl.contains("mix("), "a.glsl output missing mix()");
    }

    #[test]
    fn structural_b_glsl_spline() {
        let src = include_str!("../../../b.glsl");
        let graph = parse_glsl_to_graph(src);

        assert_count(&graph, NodeKind::GlslExpr, 0, "b.glsl");

        let helper_names: Vec<&str> = graph
            .helper_functions
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        for f in &[
            "catmullRom",
            "distanceToLineSeg",
            "debugDistanceField",
            "PointArray",
            "getUV",
        ] {
            assert!(
                helper_names.contains(f),
                "b.glsl missing \'{}\'. Found: {:?}",
                f,
                helper_names
            );
        }

        // Exact node counts
        assert_count(&graph, NodeKind::Output, 1, "b.glsl");
        assert_count(&graph, NodeKind::UV, 1, "b.glsl");
        assert_count(&graph, NodeKind::Time, 1, "b.glsl");
        assert_count(&graph, NodeKind::Mouse, 2, "b.glsl");
        assert_count(&graph, NodeKind::Mix, 6, "b.glsl");
        assert_count(&graph, NodeKind::SmoothStep, 7, "b.glsl");
        assert_count(&graph, NodeKind::Combine3, 7, "b.glsl");
        assert_count(&graph, NodeKind::Combine2, 10, "b.glsl");
        assert_count(&graph, NodeKind::Combine4, 1, "b.glsl");
        assert_count(&graph, NodeKind::CustomFunc, 6, "b.glsl");
        assert_count(&graph, NodeKind::FloatConst, 58, "b.glsl");
        assert_count(&graph, NodeKind::Subtract, 5, "b.glsl");
        assert_count(&graph, NodeKind::Multiply, 3, "b.glsl");
        assert_count(&graph, NodeKind::Add, 2, "b.glsl");
        assert_count(&graph, NodeKind::Length, 4, "b.glsl");
        assert_count(&graph, NodeKind::Fract, 3, "b.glsl");
        assert_count(&graph, NodeKind::Min, 1, "b.glsl");
        assert_count(&graph, NodeKind::Abs, 1, "b.glsl");
        assert_count(&graph, NodeKind::ForLoop, 1, "b.glsl");
        assert_count(&graph, NodeKind::Conditional, 1, "b.glsl");
        assert_count(&graph, NodeKind::GreaterThan, 1, "b.glsl");
        assert_count(&graph, NodeKind::SplitVec2, 1, "b.glsl");
        assert_count(&graph, NodeKind::SplitVec4, 7, "b.glsl");

        let nodes = graph.nodes().count();
        let conns = graph.connections().len();
        assert_eq!(nodes, 130, "b.glsl total nodes");
        assert_eq!(conns, 139, "b.glsl total connections");
        validate_connections(&graph, "b.glsl");

        let glsl = graph.compile_glsl().expect("b.glsl failed to compile");
        assert!(glsl.contains("mainImage"));
        assert!(glsl.contains("catmullRom"));
    }

    #[test]
    fn structural_c_glsl_menger() {
        let src = include_str!("../../../c.glsl");
        let graph = parse_glsl_to_graph(src);

        assert_count(&graph, NodeKind::GlslExpr, 0, "c.glsl");
        assert_count(&graph, NodeKind::Output, 1, "c.glsl");
        assert_count(&graph, NodeKind::Resolution, 1, "c.glsl");
        assert_count(&graph, NodeKind::Time, 1, "c.glsl");

        // c.glsl uses macros which are NOT expanded by the parser,
        // so actual node counts reflect only mainImage body operations
        assert_count(&graph, NodeKind::Multiply, 8, "c.glsl");
        assert_count(&graph, NodeKind::Add, 3, "c.glsl");
        assert_count(&graph, NodeKind::Subtract, 2, "c.glsl");
        assert_count(&graph, NodeKind::Divide, 3, "c.glsl");
        assert_count(&graph, NodeKind::Cos, 2, "c.glsl");
        assert_count(&graph, NodeKind::Cross, 1, "c.glsl");
        assert_count(&graph, NodeKind::Normalize, 2, "c.glsl");
        assert_count(&graph, NodeKind::Exp, 1, "c.glsl");
        assert_count(&graph, NodeKind::Negate, 2, "c.glsl");
        assert_count(&graph, NodeKind::ForLoop, 1, "c.glsl");
        assert_count(&graph, NodeKind::FloatConst, 20, "c.glsl");
        assert_count(&graph, NodeKind::CustomFunc, 3, "c.glsl");
        assert_count(&graph, NodeKind::Mat2, 1, "c.glsl");
        assert_count(&graph, NodeKind::Mat3, 1, "c.glsl");

        let nodes = graph.nodes().count();
        let conns = graph.connections().len();
        assert_eq!(nodes, 60, "c.glsl total nodes");
        assert_eq!(conns, 63, "c.glsl total connections");
        validate_connections(&graph, "c.glsl");

        let glsl = graph.compile_glsl().expect("c.glsl failed to compile");
        assert!(glsl.contains("mainImage"));
    }

    #[test]
    fn structural_d_glsl_butterflies() {
        let src = include_str!("../../../d.glsl");
        let graph = parse_glsl_to_graph(src);

        assert_count(&graph, NodeKind::GlslExpr, 0, "d.glsl");

        let helper_names: Vec<&str> = graph
            .helper_functions
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        for f in &["udBox", "sdHexPrism", "getModel", "trace"] {
            assert!(
                helper_names.contains(f),
                "d.glsl missing \'{}\'. Found: {:?}",
                f,
                helper_names
            );
        }

        assert_count(&graph, NodeKind::Output, 1, "d.glsl");
        assert_count(&graph, NodeKind::Resolution, 4, "d.glsl");
        assert_count(&graph, NodeKind::UV, 1, "d.glsl");
        assert_count(&graph, NodeKind::Mouse, 1, "d.glsl");
        assert_count(&graph, NodeKind::Time, 5, "d.glsl");
        assert_count(&graph, NodeKind::TextureSample, 2, "d.glsl");
        assert_count(&graph, NodeKind::Mix, 3, "d.glsl");
        assert_count(&graph, NodeKind::SmoothStep, 2, "d.glsl");
        assert_count(&graph, NodeKind::Multiply, 25, "d.glsl");
        assert_count(&graph, NodeKind::Add, 10, "d.glsl");
        assert_count(&graph, NodeKind::Subtract, 8, "d.glsl");
        assert_count(&graph, NodeKind::Divide, 4, "d.glsl");
        assert_count(&graph, NodeKind::Sin, 3, "d.glsl");
        assert_count(&graph, NodeKind::Cos, 1, "d.glsl");
        assert_count(&graph, NodeKind::Abs, 3, "d.glsl");
        assert_count(&graph, NodeKind::Clamp, 2, "d.glsl");
        assert_count(&graph, NodeKind::Normalize, 1, "d.glsl");
        assert_count(&graph, NodeKind::CustomFunc, 6, "d.glsl");
        assert_count(&graph, NodeKind::Conditional, 1, "d.glsl");
        assert_count(&graph, NodeKind::Equal, 1, "d.glsl");
        assert_count(&graph, NodeKind::ForLoop, 1, "d.glsl");
        assert_count(&graph, NodeKind::FloatConst, 65, "d.glsl");
        assert_count(&graph, NodeKind::Combine2, 8, "d.glsl");
        assert_count(&graph, NodeKind::Combine3, 3, "d.glsl");
        assert_count(&graph, NodeKind::Combine4, 6, "d.glsl");
        assert_count(&graph, NodeKind::SplitVec2, 1, "d.glsl");
        assert_count(&graph, NodeKind::SplitVec4, 10, "d.glsl");

        let nodes = graph.nodes().count();
        let conns = graph.connections().len();
        assert_eq!(nodes, 178, "d.glsl total nodes");
        assert_eq!(conns, 201, "d.glsl total connections");
        validate_connections(&graph, "d.glsl");

        let glsl = graph.compile_glsl().expect("d.glsl failed to compile");
        assert!(glsl.contains("mainImage"));
        assert!(glsl.contains("udBox"));
        assert!(glsl.contains("sdHexPrism"));
    }

    #[test]
    fn structural_no_glslexpr_in_example_shaders() {
        let example_shaders = &[
            include_str!("../../../examples/shadertoy/plasma.glsl"),
            include_str!("../../../examples/shadertoy/test_time.glsl"),
            include_str!("../../../examples/shadertoy/test_resolution.glsl"),
            include_str!("../../../examples/shadertoy/test_mouse.glsl"),
            include_str!("../../../examples/shadertoy/test_audio.glsl"),
        ];
        let names = &[
            "plasma",
            "test_time",
            "test_resolution",
            "test_mouse",
            "test_audio",
        ];
        for (src, name) in example_shaders.iter().zip(names.iter()) {
            let graph = parse_glsl_to_graph(src);
            assert_count(&graph, NodeKind::GlslExpr, 0, name);
            let result = graph.compile_glsl();
            assert!(
                result.is_ok(),
                "{}.glsl failed to compile: {:?}",
                name,
                result.err()
            );
            validate_connections(&graph, name);
        }
    }

    #[test]
    fn structural_comparison_nodes_from_conditionals() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float a = 0.0;
    if (uv.x > 0.5) {
        a = 1.0;
    }
    if (uv.y < 0.3) {
        a = 0.5;
    }
    if (uv.x >= 0.8) {
        a = 0.2;
    }
    fragColor = vec4(vec3(a), 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        assert_count(&graph, NodeKind::GlslExpr, 0, "conditionals");
        assert!(
            count_kind(&graph, NodeKind::GreaterThan) >= 1
                || count_kind(&graph, NodeKind::Conditional) >= 1,
            "should have GreaterThan or Conditional node"
        );
        validate_connections(&graph, "conditionals");
    }

    #[test]
    fn structural_node_labels_preserved() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float brightness = 0.5 + 0.5 * sin(iTime);
    vec3 skyColor = vec3(0.3, 0.5, 0.8);
    vec3 groundColor = vec3(0.2, 0.15, 0.1);
    vec3 col = mix(groundColor, skyColor, uv.y);
    col *= brightness;
    fragColor = vec4(col, 1.0);
}
"#;
        let graph = parse_glsl_to_graph(src);
        let labels: Vec<String> = graph.nodes().filter_map(|n| n.label.clone()).collect();
        assert!(
            labels.iter().any(|l| l == "uv"),
            "label uv missing. Labels: {:?}",
            labels
        );
        assert!(
            labels.iter().any(|l| l == "brightness"),
            "label brightness missing. Labels: {:?}",
            labels
        );
        assert!(
            labels.iter().any(|l| l == "skyColor"),
            "label skyColor missing. Labels: {:?}",
            labels
        );
        validate_connections(&graph, "label_test");
    }

    #[test]
    fn diagnostic_all_shaders() {
        for (name, src) in [
            ("b.glsl", include_str!("../../../b.glsl")),
            ("c.glsl", include_str!("../../../c.glsl")),
            ("d.glsl", include_str!("../../../d.glsl")),
        ] {
            let graph = parse_glsl_to_graph(src);
            let mut kind_counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for n in graph.nodes() {
                *kind_counts.entry(format!("{:?}", n.kind)).or_insert(0) += 1;
            }
            eprintln!("{name} dist: {kind_counts:?}");
            eprintln!(
                "{name} total: {} nodes, {} conns",
                graph.nodes().count(),
                graph.connections().len()
            );
            validate_connections(&graph, name);
            eprintln!("{name}: all {} conns validated", graph.connections().len());
        }
    }
}
