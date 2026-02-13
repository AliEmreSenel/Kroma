//! Node catalogue — port definitions, default values, and GLSL codegen for
//! every [`NodeKind`].

use std::collections::HashMap;
use crate::types::{DataType, DefaultValue, Node, NodeId, NodeKind, PortDef, PortDirection};

// ---------------------------------------------------------------------------
// Macros for concise port definitions
// ---------------------------------------------------------------------------

macro_rules! port {
    (in $name:expr, $ty:ident) => {
        PortDef { name: $name.into(), data_type: DataType::$ty, direction: PortDirection::Input }
    };
    (out $name:expr, $ty:ident) => {
        PortDef { name: $name.into(), data_type: DataType::$ty, direction: PortDirection::Output }
    };
}

// We leak Box<[PortDef]> to get &'static [PortDef] because iced requires 'static.
// This is fine — each variant is called once and cached.
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Port definitions per kind
// ---------------------------------------------------------------------------

pub fn inputs(kind: &NodeKind) -> &'static [PortDef] {
    use NodeKind::*;
    match kind {
        // --- no inputs ---
        Time | DeltaTime | Frame | Resolution | Mouse | CpuUsage | RamUsage
        | Battery | AudioLevel | UV | FloatConst | Vec2Const | Vec3Const | ColorConst => {
            &[]
        }

        // Output takes a vec4 colour
        Output => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Color", Vec4)]).as_slice()
        }

        // Binary math (A op B)
        Add | Subtract | Multiply | Divide | Power | Mod | Min | Max => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Float),
                port!(in "B", Float),
            ]).as_slice()
        }

        // Unary math
        Sqrt | Abs | Negate | Sin | Cos | Tan | Asin | Acos | Atan
        | Exp | Exp2 | Log | Log2 | Sign | Ceil | Round
        | Fract | Floor | Saturate | OneMinus | InverseSqrt => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Value", Float)]).as_slice()
        }

        // Atan2(y, x)
        Atan2 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Y", Float),
                port!(in "X", Float),
            ]).as_slice()
        }

        // Clamp(Value, Min, Max)
        Clamp => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Value", Float),
                port!(in "Min", Float),
                port!(in "Max", Float),
            ]).as_slice()
        }

        // Mix(A, B, Factor)
        Mix => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec4),
                port!(in "B", Vec4),
                port!(in "Factor", Float),
            ]).as_slice()
        }

        // Step(Edge, Value)
        Step => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Edge", Float),
                port!(in "Value", Float),
            ]).as_slice()
        }

        // SmoothStep(Edge0, Edge1, Value)
        SmoothStep => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Edge0", Float),
                port!(in "Edge1", Float),
                port!(in "Value", Float),
            ]).as_slice()
        }

        // Vector combine
        Combine2 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "X", Float),
                port!(in "Y", Float),
            ]).as_slice()
        }
        Combine3 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "X", Float),
                port!(in "Y", Float),
                port!(in "Z", Float),
            ]).as_slice()
        }
        Combine4 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "X", Float),
                port!(in "Y", Float),
                port!(in "Z", Float),
                port!(in "W", Float),
            ]).as_slice()
        }

        // Vector split
        SplitVec2 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Vec", Vec2)]).as_slice()
        }
        SplitVec3 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Vec", Vec3)]).as_slice()
        }
        SplitVec4 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Vec", Vec4)]).as_slice()
        }

        // Vector ops
        Length | Normalize => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Vec", Vec3)]).as_slice()
        }
        Dot => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec3),
                port!(in "B", Vec3),
            ]).as_slice()
        }
        Cross => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec3),
                port!(in "B", Vec3),
            ]).as_slice()
        }
        Distance => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec3),
                port!(in "B", Vec3),
            ]).as_slice()
        }
        Reflect => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "I", Vec3),
                port!(in "N", Vec3),
            ]).as_slice()
        }
        Refract => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "I", Vec3),
                port!(in "N", Vec3),
                port!(in "Eta", Float),
            ]).as_slice()
        }

        // Colour
        RgbToHsv | HsvToRgb => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(in "Color", Vec3)]).as_slice()
        }

        // Procedural
        ValueNoise | Voronoi => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "UV", Vec2),
                port!(in "Scale", Float),
            ]).as_slice()
        }

        // Custom GLSL expression: takes 4 generic vec4 inputs + UV + time
        GlslExpr => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec4),
                port!(in "B", Vec4),
                port!(in "UV", Vec2),
                port!(in "Time", Float),
            ]).as_slice()
        }

        // For Loop: Initial value, Count, Body expression
        ForLoop => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Initial", Vec4),
                port!(in "Count", Float),
                port!(in "Body", Vec4),
            ]).as_slice()
        }

        // Conditional: Condition, Threshold, TrueVal, FalseVal
        Conditional => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Cond", Float),
                port!(in "Thresh", Float),
                port!(in "True", Vec4),
                port!(in "False", Vec4),
            ]).as_slice()
        }

        // Texture sample: Channel index (float), UV
        TextureSample => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "Channel", Float),
                port!(in "UV", Vec2),
            ]).as_slice()
        }

        // Custom function: up to 4 generic inputs
        CustomFunc => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(in "A", Vec4),
                port!(in "B", Vec4),
                port!(in "C", Vec4),
                port!(in "D", Vec4),
            ]).as_slice()
        }
    }
}

pub fn outputs(kind: &NodeKind) -> &'static [PortDef] {
    use NodeKind::*;
    match kind {
        Output => &[], // terminal node

        // Single float output
        FloatConst | Time | DeltaTime | Frame | CpuUsage | RamUsage
        | Battery | AudioLevel | Length | Dot | Distance | ValueNoise | Voronoi => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Value", Float)]).as_slice()
        }

        // Vec2 output
        UV | Vec2Const | Resolution | Mouse => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Value", Vec2)]).as_slice()
        }

        // Vec3 output
        Vec3Const | Cross | Normalize | Reflect | Refract | RgbToHsv | HsvToRgb => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Value", Vec3)]).as_slice()
        }

        // Vec4 output
        ColorConst => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Color", Vec4)]).as_slice()
        }

        // Math — pass through same type (we default to float)
        Add | Subtract | Multiply | Divide | Power | Mod | Min | Max
        | Sqrt | Abs | Negate | Sin | Cos | Tan | Asin | Acos | Atan | Atan2
        | Exp | Exp2 | Log | Log2 | Sign | Ceil | Round
        | Fract | Floor | Step | Saturate | OneMinus | InverseSqrt => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Result", Float)]).as_slice()
        }
        Clamp | SmoothStep => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Result", Float)]).as_slice()
        }
        Mix => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Result", Vec4)]).as_slice()
        }

        // Combine outputs
        Combine2 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Vec", Vec2)]).as_slice()
        }
        Combine3 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Vec", Vec3)]).as_slice()
        }
        Combine4 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Vec", Vec4)]).as_slice()
        }

        // Split outputs — multiple ports
        SplitVec2 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(out "X", Float),
                port!(out "Y", Float),
            ]).as_slice()
        }
        SplitVec3 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(out "X", Float),
                port!(out "Y", Float),
                port!(out "Z", Float),
            ]).as_slice()
        }
        SplitVec4 => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![
                port!(out "X", Float),
                port!(out "Y", Float),
                port!(out "Z", Float),
                port!(out "W", Float),
            ]).as_slice()
        }

        // Custom GLSL expression outputs vec4
        GlslExpr | ForLoop | Conditional | TextureSample | CustomFunc => {
            static P: OnceLock<Vec<PortDef>> = OnceLock::new();
            P.get_or_init(|| vec![port!(out "Result", Vec4)]).as_slice()
        }
    }
}

// ---------------------------------------------------------------------------
// Default values
// ---------------------------------------------------------------------------

pub fn default_values(kind: &NodeKind) -> Vec<DefaultValue> {
    use NodeKind::*;
    match kind {
        Output => vec![DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0])],
        FloatConst => vec![DefaultValue::Float(1.0)],
        Vec2Const => vec![DefaultValue::Vec2([0.0, 0.0])],
        Vec3Const => vec![DefaultValue::Vec3([0.0, 0.0, 0.0])],
        ColorConst => vec![DefaultValue::Vec4([1.0, 0.0, 0.5, 1.0])],

        // Binary ops
        Add | Subtract | Multiply | Divide | Power | Mod | Min | Max => {
            vec![DefaultValue::Float(0.0), DefaultValue::Float(1.0)]
        }

        // Unary
        Sqrt | Abs | Negate | Sin | Cos | Tan | Asin | Acos | Atan
        | Exp | Exp2 | Log | Log2 | Sign | Ceil | Round
        | Fract | Floor | Saturate | OneMinus | InverseSqrt => {
            vec![DefaultValue::Float(0.0)]
        }

        // Atan2
        Atan2 => vec![DefaultValue::Float(0.0), DefaultValue::Float(1.0)],

        Clamp => vec![DefaultValue::Float(0.0), DefaultValue::Float(0.0), DefaultValue::Float(1.0)],
        Mix => vec![
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Vec4([1.0, 1.0, 1.0, 1.0]),
            DefaultValue::Float(0.5),
        ],
        Step => vec![DefaultValue::Float(0.5), DefaultValue::Float(0.0)],
        SmoothStep => vec![DefaultValue::Float(0.0), DefaultValue::Float(1.0), DefaultValue::Float(0.5)],

        Combine2 => vec![DefaultValue::Float(0.0); 2],
        Combine3 => vec![DefaultValue::Float(0.0); 3],
        Combine4 => vec![DefaultValue::Float(0.0); 4],

        SplitVec2 => vec![DefaultValue::Vec2([0.0, 0.0])],
        SplitVec3 => vec![DefaultValue::Vec3([0.0, 0.0, 0.0])],
        SplitVec4 => vec![DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0])],

        Length | Normalize => vec![DefaultValue::Vec3([0.0, 0.0, 1.0])],
        Dot | Cross | Distance | Reflect => vec![
            DefaultValue::Vec3([1.0, 0.0, 0.0]),
            DefaultValue::Vec3([0.0, 1.0, 0.0]),
        ],
        Refract => vec![
            DefaultValue::Vec3([1.0, 0.0, 0.0]),
            DefaultValue::Vec3([0.0, 1.0, 0.0]),
            DefaultValue::Float(0.5),
        ],

        RgbToHsv | HsvToRgb => vec![DefaultValue::Vec3([1.0, 0.0, 0.0])],

        ValueNoise | Voronoi => vec![
            DefaultValue::Vec2([0.0, 0.0]),
            DefaultValue::Float(10.0),
        ],

        // Custom GLSL expression: A (vec4), B (vec4), UV (vec2), Time (float)
        GlslExpr => vec![
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Vec4([1.0, 1.0, 1.0, 1.0]),
            DefaultValue::Vec2([0.0, 0.0]),
            DefaultValue::Float(0.0),
        ],

        ForLoop => vec![
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Float(5.0),
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
        ],

        Conditional => vec![
            DefaultValue::Float(0.5),
            DefaultValue::Float(0.5),
            DefaultValue::Vec4([1.0, 1.0, 1.0, 1.0]),
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
        ],

        TextureSample => vec![
            DefaultValue::Float(0.0),
            DefaultValue::Vec2([0.0, 0.0]),
        ],

        CustomFunc => vec![
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
            DefaultValue::Vec4([0.0, 0.0, 0.0, 1.0]),
        ],

        // Uniform nodes have no inputs
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// GLSL code generation
// ---------------------------------------------------------------------------

/// Infer the GLSL type name from the node's defaults.
/// Defaults to `"float"` if no defaults are available.
fn infer_type(defaults: &[DefaultValue]) -> &'static str {
    defaults.first()
        .map(|d| d.data_type().glsl_type())
        .unwrap_or("float")
}

/// Generate a GLSL statement for one node.  `inputs` are the variable names
/// (or expressions) feeding each input port.  `var` is the output variable name
/// this node should write to.  `defaults` are the node's configurable default values.
/// `meta` is optional metadata (GlslExpr: expression template, CustomFunc: function name).
///
/// Returns a string like `float n3 = sin(n1);`.
pub fn codegen(kind: &NodeKind, inputs: &[String], var: &str, defaults: &[DefaultValue], meta: &Option<String>) -> String {
    use NodeKind::*;
    match kind {
        // Output — handled specially in compile_glsl
        Output => String::new(),

        // Constants — use defaults when available
        FloatConst => {
            let val = defaults.first().map(|d| d.to_glsl()).unwrap_or_else(|| "1.0".into());
            format!("float {} = {};", var, val)
        }
        Vec2Const => {
            let val = defaults.first().map(|d| d.to_glsl()).unwrap_or_else(|| "vec2(0.0, 0.0)".into());
            format!("vec2 {} = {};", var, val)
        }
        Vec3Const => {
            let val = defaults.first().map(|d| d.to_glsl()).unwrap_or_else(|| "vec3(0.0, 0.0, 0.0)".into());
            format!("vec3 {} = {};", var, val)
        }
        ColorConst => {
            let val = defaults.first().map(|d| d.to_glsl()).unwrap_or_else(|| "vec4(1.0, 0.0, 0.5, 1.0)".into());
            format!("vec4 {} = {};", var, val)
        }

        // Kroma uniforms
        Time => format!("float {} = iTime;", var),
        DeltaTime => format!("float {} = iTimeDelta;", var),
        Frame => format!("float {} = float(iFrame);", var),
        Resolution => format!("vec2 {} = iResolution.xy;", var),
        Mouse => format!("vec2 {} = iMouse.xy / iResolution.xy;", var),
        CpuUsage => format!("float {} = u_cpu;", var),
        RamUsage => format!("float {} = u_ram;", var),
        Battery => format!("float {} = u_battery;", var),
        AudioLevel => format!("float {} = u_audio_level;", var),
        UV => format!("vec2 {} = fragCoord / iResolution.xy;", var),

        // Binary math — type inferred from defaults
        Add => { let t = infer_type(defaults); format!("{t} {var} = {a} + {b};", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Subtract => { let t = infer_type(defaults); format!("{t} {var} = {a} - {b};", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Multiply => { let t = infer_type(defaults); format!("{t} {var} = {a} * {b};", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Divide => { let t = infer_type(defaults); format!("{t} {var} = {a} / (abs({b}) < 0.0001 ? 0.0001 : {b});", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Power => format!("float {} = pow({}, {});", var, a(inputs, 0), a(inputs, 1)),
        Mod => { let t = infer_type(defaults); format!("{t} {var} = mod({a}, {b});", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Min => { let t = infer_type(defaults); format!("{t} {var} = min({a}, {b});", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }
        Max => { let t = infer_type(defaults); format!("{t} {var} = max({a}, {b});", t=t, var=var, a=a(inputs, 0), b=a(inputs, 1)) }

        // Unary math — type inferred from defaults
        Sqrt => { let t = infer_type(defaults); format!("{} {} = sqrt(abs({}));", t, var, a(inputs, 0)) }
        Abs => { let t = infer_type(defaults); format!("{} {} = abs({});", t, var, a(inputs, 0)) }
        Negate => { let t = infer_type(defaults); format!("{} {} = -{};", t, var, a(inputs, 0)) }
        Sin => { let t = infer_type(defaults); format!("{} {} = sin({});", t, var, a(inputs, 0)) }
        Cos => { let t = infer_type(defaults); format!("{} {} = cos({});", t, var, a(inputs, 0)) }
        Tan => { let t = infer_type(defaults); format!("{} {} = tan({});", t, var, a(inputs, 0)) }
        Asin => { let t = infer_type(defaults); format!("{} {} = asin(clamp({}, -1.0, 1.0));", t, var, a(inputs, 0)) }
        Acos => { let t = infer_type(defaults); format!("{} {} = acos(clamp({}, -1.0, 1.0));", t, var, a(inputs, 0)) }
        Atan => { let t = infer_type(defaults); format!("{} {} = atan({});", t, var, a(inputs, 0)) }
        Atan2 => format!("float {} = atan({}, {});", var, a(inputs, 0), a(inputs, 1)),
        Exp => { let t = infer_type(defaults); format!("{} {} = exp({});", t, var, a(inputs, 0)) }
        Exp2 => { let t = infer_type(defaults); format!("{} {} = exp2({});", t, var, a(inputs, 0)) }
        Log => { let t = infer_type(defaults); format!("{} {} = log(max({}, 0.0001));", t, var, a(inputs, 0)) }
        Log2 => { let t = infer_type(defaults); format!("{} {} = log2(max({}, 0.0001));", t, var, a(inputs, 0)) }
        Sign => { let t = infer_type(defaults); format!("{} {} = sign({});", t, var, a(inputs, 0)) }
        Ceil => { let t = infer_type(defaults); format!("{} {} = ceil({});", t, var, a(inputs, 0)) }
        Round => { let t = infer_type(defaults); format!("{} {} = floor({} + 0.5);", t, var, a(inputs, 0)) }
        Fract => { let t = infer_type(defaults); format!("{} {} = fract({});", t, var, a(inputs, 0)) }
        Floor => { let t = infer_type(defaults); format!("{} {} = floor({});", t, var, a(inputs, 0)) }
        Saturate => { let t = infer_type(defaults); format!("{} {} = clamp({}, 0.0, 1.0);", t, var, a(inputs, 0)) }
        OneMinus => { let t = infer_type(defaults); format!("{} {} = 1.0 - {};", t, var, a(inputs, 0)) }
        InverseSqrt => { let t = infer_type(defaults); format!("{} {} = inversesqrt(max({}, 0.0001));", t, var, a(inputs, 0)) },

        // Ternary
        Clamp => { let t = infer_type(defaults); format!("{} {} = clamp({}, {}, {});", t, var, a(inputs, 0), a(inputs, 1), a(inputs, 2)) }
        Mix => { let t = infer_type(defaults); format!("{} {} = mix({}, {}, {});", t, var, a(inputs, 0), a(inputs, 1), a(inputs, 2)) }
        Step => { let t = infer_type(defaults); format!("{} {} = step({}, {});", t, var, a(inputs, 0), a(inputs, 1)) }
        SmoothStep => { let t = infer_type(defaults); format!("{} {} = smoothstep({}, {}, {});", t, var, a(inputs, 0), a(inputs, 1), a(inputs, 2)) }

        // Combine
        Combine2 => format!("vec2 {} = vec2({}, {});", var, a(inputs, 0), a(inputs, 1)),
        Combine3 => format!("vec3 {} = vec3({}, {}, {});", var, a(inputs, 0), a(inputs, 1), a(inputs, 2)),
        Combine4 => format!("vec4 {} = vec4({}, {}, {}, {});", var, a(inputs, 0), a(inputs, 1), a(inputs, 2), a(inputs, 3)),

        // Split — multi-output: emit separate variables
        SplitVec2 => format!("float {}_o0 = {}.x; float {}_o1 = {}.y;", var, a(inputs, 0), var, a(inputs, 0)),
        SplitVec3 => format!(
            "float {v}_o0 = {i}.x; float {v}_o1 = {i}.y; float {v}_o2 = {i}.z;",
            v = var, i = a(inputs, 0)
        ),
        SplitVec4 => format!(
            "float {v}_o0 = {i}.x; float {v}_o1 = {i}.y; float {v}_o2 = {i}.z; float {v}_o3 = {i}.w;",
            v = var, i = a(inputs, 0)
        ),

        // Vector ops
        Length => format!("float {} = length({});", var, a(inputs, 0)),
        Normalize => { let t = infer_type(defaults); format!("{} {} = normalize({});", t, var, a(inputs, 0)) }
        Dot => format!("float {} = dot({}, {});", var, a(inputs, 0), a(inputs, 1)),
        Cross => { let t = infer_type(defaults); format!("{} {} = cross({}, {});", t, var, a(inputs, 0), a(inputs, 1)) }
        Distance => format!("float {} = distance({}, {});", var, a(inputs, 0), a(inputs, 1)),
        Reflect => { let t = infer_type(defaults); format!("{} {} = reflect({}, {});", t, var, a(inputs, 0), a(inputs, 1)) }
        Refract => { let t = infer_type(defaults); format!("{} {} = refract({}, {}, {});", t, var, a(inputs, 0), a(inputs, 1), a(inputs, 2)) }

        // Colour
        RgbToHsv => format!("vec3 {} = kroma_rgb2hsv({});", var, a(inputs, 0)),
        HsvToRgb => format!("vec3 {} = kroma_hsv2rgb({});", var, a(inputs, 0)),

        // Procedural
        ValueNoise => format!("float {} = kroma_vnoise({} * {});", var, a(inputs, 0), a(inputs, 1)),
        Voronoi => format!("float {} = kroma_voronoi({} * {});", var, a(inputs, 0), a(inputs, 1)),

        // Custom GLSL — use stored expression template if available
        GlslExpr => {
            if let Some(template) = meta {
                // Template uses {A} and {B} as placeholders
                let expr = template
                    .replace("{A}", a(inputs, 0))
                    .replace("{B}", a(inputs, 1));
                // Comparison / boolean / bitwise operators produce scalar results
                let is_scalar_op = ["==", "!=", ">", "<", ">=", "<=", "&&", "||", "!", "~", "&", "|", "^"]
                    .iter().any(|op| template.contains(op));
                if is_scalar_op {
                    format!("float {} = float({});", var, expr)
                } else {
                    format!("vec4 {} = vec4({});", var, expr)
                }
            } else {
                format!(
                    "vec4 {} = mix({}, {}, vec4(length({}) * sin({}) * 0.5 + 0.5));",
                    var, a(inputs, 0), a(inputs, 1), a(inputs, 2), a(inputs, 3)
                )
            }
        }

        // For loop: iterate count times, accumulating into result
        ForLoop => format!(
            "vec4 {v} = {init}; {{ float _kfl = max({count}, 0.0001); for (int i = 0; i < int({count}); i++) {{ {v} += {body} / _kfl; }} }}",
            v = var, init = a(inputs, 0), count = a(inputs, 1), body = a(inputs, 2)
        ),

        // Conditional: step-based branch (branchless for GPU)
        Conditional => format!(
            "vec4 {v} = mix({f}, {t}, step({thresh}, {cond}));",
            v = var, cond = a(inputs, 0), thresh = a(inputs, 1), t = a(inputs, 2), f = a(inputs, 3)
        ),

        // Texture sample — channel index is the integer part of the default
        TextureSample => {
            let ch_idx = defaults.first()
                .map(|d| d.to_glsl())
                .and_then(|s| s.parse::<f32>().ok())
                .map(|f| f as i32)
                .unwrap_or(0);
            format!(
                "vec4 {v} = texture(iChannel{ch}, {uv});",
                v = var, ch = ch_idx, uv = a(inputs, 1)
            )
        },

        // Custom function call — use stored function name if available
        CustomFunc => {
            if let Some(func_name) = meta {
                // Emit a call to the named function with available args
                let arg_list: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
                let args_str = arg_list.join(", ");
                format!("vec4 {v} = {fn_name}({args});",
                    v = var, fn_name = func_name, args = args_str)
            } else {
                format!(
                    "vec4 {v} = vec4({a}, {b}, {c}, {d});",
                    v = var, a = a(inputs, 0), b = a(inputs, 1), c = a(inputs, 2), d = a(inputs, 3)
                )
            }
        },
    }
}

fn a(inputs: &[String], index: usize) -> &str {
    inputs.get(index).map(|s| s.as_str()).unwrap_or("0.0")
}

// ---------------------------------------------------------------------------
// GLSL helper functions (emitted once when needed)
// ---------------------------------------------------------------------------

pub fn helper_functions(nodes: &HashMap<NodeId, Node>) -> String {
    let mut out = String::new();
    let has = |k: &NodeKind| nodes.values().any(|n| &n.kind == k);

    if has(&NodeKind::RgbToHsv) {
        out.push_str(
"vec3 kroma_rgb2hsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0/3.0, 2.0/3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}\n\n",
        );
    }

    if has(&NodeKind::HsvToRgb) {
        out.push_str(
"vec3 kroma_hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0/3.0, 1.0/3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}\n\n",
        );
    }

    if has(&NodeKind::ValueNoise) {
        out.push_str(
"float kroma_hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}
float kroma_vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    float a = kroma_hash(i);
    float b = kroma_hash(i + vec2(1.0, 0.0));
    float c = kroma_hash(i + vec2(0.0, 1.0));
    float d = kroma_hash(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}\n\n",
        );
    }

    if has(&NodeKind::Voronoi) && !has(&NodeKind::ValueNoise) {
        // Emit kroma_hash dependency if ValueNoise hasn't already emitted it
        out.push_str(
"float kroma_hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}
");
    }

    if has(&NodeKind::Voronoi) {
        out.push_str(
"float kroma_voronoi(vec2 p) {
    vec2 n = floor(p);
    vec2 f = fract(p);
    float md = 8.0;
    for (int j = -1; j <= 1; j++) {
        for (int i = -1; i <= 1; i++) {
            vec2 g = vec2(float(i), float(j));
            vec2 o = vec2(kroma_hash(n + g));
            vec2 r = g + o - f;
            float d = dot(r, r);
            md = min(md, d);
        }
    }
    return sqrt(md);
}\n\n",
        );
    }

    out
}
