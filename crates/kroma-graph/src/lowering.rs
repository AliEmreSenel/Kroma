//! AST → node-graph lowering.
//!
//! Uses the `glsl` crate to parse Shadertoy-style GLSL into a
//! [`glsl::syntax::TranslationUnit`], then walks the AST to produce
//! a [`ShaderGraph`] with properly wired nodes.

use std::collections::HashMap;

use glsl::parser::Parse;
use glsl::syntax::{
    self, BinaryOp, CompoundStatement, Declaration, Expr, ExternalDeclaration,
    ForInitStatement, ForRestStatement, FunIdentifier,
    FunctionDefinition, Identifier, InitDeclaratorList, Initializer, IterationStatement,
    JumpStatement, SelectionRestStatement, SelectionStatement, SimpleStatement,
    Statement, TranslationUnit, UnaryOp,
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

    eprintln!("[kroma-graph] Parsing GLSL ({} chars, prepared {} chars)…",
              src.len(), prepped.len());

    let tu = match TranslationUnit::parse(&prepped) {
        Ok(tu) => {
            eprintln!("[kroma-graph] GLSL parsed OK — {} top-level declarations",
                      tu.0.0.len());
            // Log the AST structure
            for decl in &tu.0 {
                eprintln!("[kroma-graph]   AST decl: {:#?}", decl);
            }
            tu
        }
        Err(e) => {
            eprintln!("[kroma-graph] GLSL parse FAILED: {:?}", e);
            return ShaderGraph::new();
        }
    };

    let mut ctx = LowerCtx::new();
    ctx.lower_translation_unit(&tu);

    let node_count = ctx.graph.nodes().count();
    let conn_count = ctx.graph.connections().len();
    eprintln!("[kroma-graph] Lowered to graph: {} nodes, {} connections",
              node_count, conn_count);

    // Auto-layout nodes based on dependency tree
    ctx.graph.auto_layout();

    // Print node summary (after layout)
    for node in ctx.graph.nodes() {
        eprintln!("[kroma-graph]   {:?} → {:?} at ({:.0}, {:.0})",
                  node.id, node.kind, node.position[0], node.position[1]);
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

    for line in src.lines() {
        let trimmed = line.trim_start();

        // Skip preprocessor directives (#version, #define, etc.)
        if trimmed.starts_with('#') {
            continue;
        }
        // Skip layout(...) declarations (uniform blocks, sampler bindings, outputs)
        if trimmed.starts_with("layout(") || trimmed.starts_with("layout (") {
            continue;
        }
        // Skip Kroma auto-generated comment blocks
        if trimmed.starts_with("// ====") || trimmed.starts_with("// Auto-generated") {
            continue;
        }
        // Skip uniform block contents (inside the Globals block)
        if trimmed.starts_with("uniform ") {
            continue;
        }
        // Skip closing brace of uniform block followed by nothing (};)
        if trimmed == "};" {
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
                !(t.starts_with("vec2 fragCoord =") || t.starts_with("vec4 fragColor ="))
                // Skip kroma_out_color assignment
                && !t.starts_with("kroma_out_color =")
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Remove the wrapper main() that calls kroma_main()
        // (it no longer exists after the rename)
    }

    // Remove the `void main() { kroma_main(); }` wrapper if still present
    stripped = stripped.replace("void main() {\n    kroma_main();\n}", "");
    stripped = stripped.replace("void main() {\n    mainImage();\n}", "");

    // If it already has mainImage, use it directly.
    if stripped.contains("mainImage") {
        return stripped;
    }

    // Wrap bare code in a mainImage function.
    format!(
        "void mainImage(out vec4 fragColor, in vec2 fragCoord) {{\n{stripped}\n}}"
    )
}

// ---------------------------------------------------------------------------
// Lowering context
// ---------------------------------------------------------------------------

struct LowerCtx {
    graph: ShaderGraph,
    /// Maps variable names → (node_id, output_port) of the node that produces
    /// that variable's value.
    vars: HashMap<String, PortAddr>,
    /// Layout column counter for auto-placement.
    col: f32,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            graph: ShaderGraph::new(),
            vars: HashMap::new(),
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
        }
        // TODO: helper functions → CustomFunc nodes
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
            SimpleStatement::Expression(opt_expr) => {
                if let Some(e) = opt_expr {
                    self.lower_expr(e);
                }
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
        let sd = &idl.head;
        if let Some(name) = &sd.name {
            let var_name = name.0.clone();
            if let Some(Initializer::Simple(init_expr)) = &sd.initializer {
                let src = self.lower_expr(init_expr);
                self.vars.insert(var_name, src);
            }
        }
        // Tail declarations (multiple in one line)
        for tail in &idl.tail {
            let var_name = tail.ident.ident.0.clone();
            if let Some(Initializer::Simple(init_expr)) = &tail.initializer {
                let src = self.lower_expr(init_expr);
                self.vars.insert(var_name, src);
            }
        }
    }

    // -- Selection (if/else → Conditional node) ----------------------------

    fn lower_selection(&mut self, sel: &SelectionStatement) {
        let cond_src = self.lower_expr(&sel.cond);
        // We create a Conditional node.  We'll lower the then/else bodies
        // as sub-expressions if they're simple assignments.
        let node_id = self.add_node(NodeKind::Conditional);

        // Connect cond
        self.connect(cond_src, PortAddr { node: node_id, port: 0 });

        match &sel.rest {
            SelectionRestStatement::Statement(then_stmt) => {
                // try to extract the value from the then branch
                if let Some(val) = self.extract_branch_value(then_stmt) {
                    self.connect(val, PortAddr { node: node_id, port: 1 }); // "True"
                }
            }
            SelectionRestStatement::Else(then_stmt, else_stmt) => {
                if let Some(val) = self.extract_branch_value(then_stmt) {
                    self.connect(val, PortAddr { node: node_id, port: 1 });
                }
                if let Some(val) = self.extract_branch_value(else_stmt) {
                    self.connect(val, PortAddr { node: node_id, port: 2 }); // "False"
                }
            }
        }

        // Conditional output is port 0
        // If the branch assigns to a known variable, update the var map.
        // For now we just keep the node available.
    }

    fn extract_branch_value(&mut self, stmt: &Statement) -> Option<PortAddr> {
        match stmt {
            Statement::Simple(boxed) => match boxed.as_ref() {
                SimpleStatement::Expression(Some(Expr::Assignment(_lhs, _, rhs))) => {
                    Some(self.lower_expr(rhs))
                }
                SimpleStatement::Expression(Some(e)) => {
                    Some(self.lower_expr(e))
                }
                _ => None,
            },
            Statement::Compound(cs) => {
                // Take the last statement's value
                cs.statement_list.last().and_then(|s| self.extract_branch_value(s))
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
                    self.connect(src, PortAddr { node: node_id, port: 0 }); // init
                }
                if let Some(val) = self.extract_branch_value(body) {
                    self.connect(val, PortAddr { node: node_id, port: 2 }); // body
                }
            }
            IterationStatement::DoWhile(body, _cond) => {
                // Same strategy
                let node_id = self.add_node(NodeKind::ForLoop);
                if let Some(val) = self.extract_branch_value(body) {
                    self.connect(val, PortAddr { node: node_id, port: 2 });
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
                if let Declaration::InitDeclaratorList(idl) = boxed_decl.as_ref() {
                if let Some(Initializer::Simple(init_expr)) = &idl.head.initializer {
                    let src = self.lower_expr(init_expr);
                    self.connect(src, PortAddr { node: node_id, port: 0 }); // "Init"

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
            }
            ForInitStatement::Expression(Some(e)) => {
                let src = self.lower_expr(e);
                self.connect(src, PortAddr { node: node_id, port: 0 });
            }
            _ => {}
        }

        // Condition: try to extract the upper bound as "Count"
        if let Some(syntax::Condition::Expr(cond_expr)) = &rest.condition {
            if let Some(count_src) = self.extract_loop_bound(cond_expr) {
                self.connect(count_src, PortAddr { node: node_id, port: 1 }); // "Count"
            }
        }

        // Body: lower and connect
        if let Some(val) = self.extract_branch_value(body) {
            self.connect(val, PortAddr { node: node_id, port: 2 }); // "Body"
        }
    }

    /// Try to extract the upper bound from `i < N` or `i <= N`.
    fn extract_loop_bound(&mut self, cond: &Expr) -> Option<PortAddr> {
        match cond {
            Expr::Binary(BinaryOp::LT | BinaryOp::LTE, _lhs, rhs) => {
                Some(self.lower_expr(rhs))
            }
            Expr::Binary(BinaryOp::GT | BinaryOp::GTE, lhs, _rhs) => {
                Some(self.lower_expr(lhs))
            }
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
            self.connect(src, PortAddr { node: output_id, port: 0 });
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
            Expr::Assignment(lhs, _op, rhs) => {
                let rhs_src = self.lower_expr(rhs);
                // Update var map if lhs is a variable
                if let Expr::Variable(ident) = lhs.as_ref() {
                    self.vars.insert(ident.0.clone(), rhs_src);

                    // If this is fragColor, connect to Output
                    if is_frag_color(&ident.0) {
                        self.connect(rhs_src, PortAddr { node: NodeId(1), port: 0 });
                    }
                }
                rhs_src
            }

            // -- PostInc / PostDec (treat as identity for graph) ---
            Expr::PostInc(e) | Expr::PostDec(e) => self.lower_expr(e),

            // -- Comma ---
            Expr::Comma(_a, b) => self.lower_expr(b),

            // -- Bracket (array access — fallback to GlslExpr) ---
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
                // Unknown variable — create a float const with 0.0
                self.make_float_const(0.0)
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
            // Comparison ops → we don't have dedicated nodes; use GlslExpr
            _ => return self.make_glsl_fallback_binop(op, lhs, rhs),
        };

        let node_id = self.add_node(kind);
        self.lower_arg_or_default(lhs, node_id, 0);
        self.lower_arg_or_default(rhs, node_id, 1);
        PortAddr { node: node_id, port: 0 }
    }

    fn make_glsl_fallback_binop(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> PortAddr {
        // For comparison ops we can use a Conditional node or just a GlslExpr
        let _op_str = match op {
            BinaryOp::LT => "<",
            BinaryOp::GT => ">",
            BinaryOp::LTE => "<=",
            BinaryOp::GTE => ">=",
            BinaryOp::Equal => "==",
            BinaryOp::NonEqual => "!=",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::LShift => "<<",
            BinaryOp::RShift => ">>",
            _ => "?",
        };
        let l = self.lower_expr(lhs);
        let r = self.lower_expr(rhs);
        let node_id = self.add_node(NodeKind::GlslExpr);
        // Set the expression string as a default
        if let Some(node) = self.graph.node_mut(node_id) {
            if !node.defaults.is_empty() {
                // GlslExpr has a Vec4 default; we leave it and just connect inputs
            }
        }
        self.connect(l, PortAddr { node: node_id, port: 0 });
        self.connect(r, PortAddr { node: node_id, port: 1 });
        PortAddr { node: node_id, port: 0 }
    }

    // -- Unary ops ---------------------------------------------------------

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expr) -> PortAddr {
        let kind = match op {
            UnaryOp::Minus => NodeKind::Negate,
            UnaryOp::Add => return self.lower_expr(operand), // +x = x
            UnaryOp::Not | UnaryOp::Complement => {
                // No dedicated node — GlslExpr fallback
                let src = self.lower_expr(operand);
                let node_id = self.add_node(NodeKind::GlslExpr);
                self.connect(src, PortAddr { node: node_id, port: 0 });
                return PortAddr { node: node_id, port: 0 };
            }
            UnaryOp::Inc | UnaryOp::Dec => return self.lower_expr(operand),
        };

        let node_id = self.add_node(kind);
        self.lower_arg_or_default(operand, node_id, 0);
        PortAddr { node: node_id, port: 0 }
    }

    // -- Ternary -----------------------------------------------------------

    fn lower_ternary(&mut self, cond: &Expr, t: &Expr, f: &Expr) -> PortAddr {
        let node_id = self.add_node(NodeKind::Conditional);
        self.lower_arg_or_default(cond, node_id, 0);
        self.lower_arg_or_default(t, node_id, 1);
        self.lower_arg_or_default(f, node_id, 2);
        PortAddr { node: node_id, port: 0 }
    }

    // -- Function calls / type constructors --------------------------------

    fn lower_fun_call(&mut self, fun_id: &FunIdentifier, args: &[Expr]) -> PortAddr {
        let name = match fun_id {
            FunIdentifier::Identifier(ident) => ident.0.as_str(),
            FunIdentifier::Expr(e) => {
                // Rare: function pointer or computed call
                return self.lower_expr(e);
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
                "atan" => {
                    Some(NodeKind::Atan)
                }
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
                return PortAddr { node: node_id, port: 0 };
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
                return PortAddr { node: node_id, port: 0 };
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
                return PortAddr { node: node_id, port: 0 };
            }
        }

        // Texture sampling
        if name == "texture" || name.starts_with("texture2D") {
            let node_id = self.add_node(NodeKind::TextureSample);
            if args.len() >= 2 {
                let uv = self.lower_expr(&args[1]);
                self.connect(uv, PortAddr { node: node_id, port: 0 });
            }
            return PortAddr { node: node_id, port: 0 };
        }

        // Fallback: CustomFunc node
        let node_id = self.add_node(NodeKind::CustomFunc);
        for (i, arg) in args.iter().enumerate() {
            let src = self.lower_expr(arg);
            // CustomFunc has input ports 0..n
            self.connect(src, PortAddr { node: node_id, port: i });
        }
        PortAddr { node: node_id, port: 0 }
    }

    // -- Vector constructors -----------------------------------------------

    fn lower_combine(&mut self, components: usize, args: &[Expr]) -> PortAddr {
        // If all args are simple float constants, create a VecNConst
        if args.len() == 1 {
            // vec3(x) → broadcast
            let kind = match components {
                2 => NodeKind::Combine2,
                3 => NodeKind::Combine3,
                4 => NodeKind::Combine4,
                _ => return self.lower_expr(&args[0]),
            };
            let node_id = self.add_node(kind);
            // If the single arg is a literal, set it as default on all ports
            if let Some(val) = Self::try_as_float_literal(&args[0]) {
                if let Some(node) = self.graph.node_mut(node_id) {
                    for i in 0..components.min(node.defaults.len()) {
                        node.defaults[i] = DefaultValue::Float(val);
                    }
                }
            } else {
                let src = self.lower_expr(&args[0]);
                for i in 0..components {
                    self.connect(src, PortAddr { node: node_id, port: i });
                }
            }
            return PortAddr { node: node_id, port: 0 };
        }

        let kind = match components {
            2 => NodeKind::Combine2,
            3 => NodeKind::Combine3,
            4 => NodeKind::Combine4,
            _ => return self.make_float_const(0.0),
        };

        let node_id = self.add_node(kind);

        // Handle mixed-size args: vec4(vec2, float, float), vec4(vec3, float), etc.
        let mut port_idx = 0;
        for arg in args {
            if port_idx >= components {
                break;
            }
            self.lower_arg_or_default(arg, node_id, port_idx);
            port_idx += 1;
        }

        PortAddr { node: node_id, port: 0 }
    }

    // -- Dot / swizzle -----------------------------------------------------

    fn lower_dot(&mut self, object: &Expr, field: &Identifier) -> PortAddr {
        let field_str = &field.0;

        // iResolution.xy → UV pattern
        if let Expr::Variable(ident) = object {
            if is_resolution(&ident.0) && (field_str == "xy" || field_str == "x" || field_str == "y") {
                let id = self.add_node(NodeKind::Resolution);
                return PortAddr { node: id, port: 0 };
            }
        }

        let obj_src = self.lower_expr(object);

        // Swizzle: .x, .y, .z, .w, .r, .g, .b, .a, .xy, .xyz, etc.
        if is_swizzle(field_str) {
            let n_components = field_str.len();
            if n_components == 1 {
                // Single component → SplitVec
                let split_kind = NodeKind::SplitVec4; // conservative
                let node_id = self.add_node(split_kind);
                self.connect(obj_src, PortAddr { node: node_id, port: 0 });
                let component_port = match field_str.chars().next() {
                    Some('x' | 'r' | 's') => 0,
                    Some('y' | 'g' | 't') => 1,
                    Some('z' | 'b' | 'p') => 2,
                    Some('w' | 'a' | 'q') => 3,
                    _ => 0,
                };
                return PortAddr { node: node_id, port: component_port };
            }
            // Multi-component swizzle → just pass through for now
            return obj_src;
        }

        // Non-swizzle member access — just pass through
        obj_src
    }

    // -- Helpers -----------------------------------------------------------

    /// If `expr` is a numeric literal, return the value as f32.
    fn try_as_float_literal(expr: &Expr) -> Option<f32> {
        match expr {
            Expr::FloatConst(f) => Some(*f),
            Expr::DoubleConst(d) => Some(*d as f32),
            Expr::IntConst(i) => Some(*i as f32),
            Expr::UIntConst(u) => Some(*u as f32),
            Expr::BoolConst(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// Lower an argument destined for `target_node` at `target_port`.
    /// If the argument is a literal float, set it as the inline default
    /// instead of creating a FloatConst node + wire.
    fn lower_arg_or_default(&mut self, arg: &Expr, target_node: NodeId, target_port: usize) {
        if let Some(val) = Self::try_as_float_literal(arg) {
            if let Some(node) = self.graph.node_mut(target_node) {
                if target_port < node.defaults.len() {
                    node.defaults[target_port] = DefaultValue::Float(val);
                }
            }
        } else {
            let src = self.lower_expr(arg);
            self.connect(src, PortAddr { node: target_node, port: target_port });
        }
    }

    fn make_float_const(&mut self, val: f64) -> PortAddr {
        let node_id = self.add_node(NodeKind::FloatConst);
        // Set the default value
        if let Some(node) = self.graph.node_mut(node_id) {
            if !node.defaults.is_empty() {
                node.defaults[0] = DefaultValue::Float(val as f32);
            }
        }
        PortAddr { node: node_id, port: 0 }
    }

    fn make_glsl_fallback(&mut self, _expr: &Expr) -> PortAddr {
        let node_id = self.add_node(NodeKind::GlslExpr);
        PortAddr { node: node_id, port: 0 }
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
        && field
            .chars()
            .all(|c| matches!(c, 'x' | 'y' | 'z' | 'w' | 'r' | 'g' | 'b' | 'a' | 's' | 't' | 'p' | 'q'))
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
}
