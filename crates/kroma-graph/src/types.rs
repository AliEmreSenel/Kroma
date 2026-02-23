//! Core types for the shader node graph.

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique identifier for a node within a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies a specific port on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortAddr {
    pub node: NodeId,
    pub port: usize,
}

/// Unique ID for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub u64);

// ---------------------------------------------------------------------------
// Port / value types
// ---------------------------------------------------------------------------

/// The data type carried by a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Float,
    Vec2,
    Vec3,
    Vec4,
}

impl DataType {
    pub fn glsl_type(&self) -> &'static str {
        match self {
            DataType::Float => "float",
            DataType::Vec2 => "vec2",
            DataType::Vec3 => "vec3",
            DataType::Vec4 => "vec4",
        }
    }

    /// Colour used when drawing this type's ports and wires.
    pub fn color(&self) -> [f32; 3] {
        match self {
            DataType::Float => [0.65, 0.85, 0.65],
            DataType::Vec2 => [0.55, 0.75, 1.0],
            DataType::Vec3 => [1.0, 0.75, 0.45],
            DataType::Vec4 => [0.85, 0.55, 0.85],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
}

/// A port descriptor (template — part of the node catalogue).
#[derive(Debug, Clone)]
pub struct PortDef {
    pub name: String,
    pub data_type: DataType,
    pub direction: PortDirection,
}

// ---------------------------------------------------------------------------
// Default values
// ---------------------------------------------------------------------------

/// A concrete default value held by an unconnected input port.
#[derive(Debug, Clone)]
pub enum DefaultValue {
    Float(f32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
}

impl DefaultValue {
    pub fn to_glsl(&self) -> String {
        match self {
            DefaultValue::Float(v) => format!("{:.6}", v),
            DefaultValue::Vec2(v) => format!("vec2({:.6}, {:.6})", v[0], v[1]),
            DefaultValue::Vec3(v) => format!("vec3({:.6}, {:.6}, {:.6})", v[0], v[1], v[2]),
            DefaultValue::Vec4(v) => {
                format!("vec4({:.6}, {:.6}, {:.6}, {:.6})", v[0], v[1], v[2], v[3])
            }
        }
    }

    pub fn data_type(&self) -> DataType {
        match self {
            DefaultValue::Float(_) => DataType::Float,
            DefaultValue::Vec2(_) => DataType::Vec2,
            DefaultValue::Vec3(_) => DataType::Vec3,
            DefaultValue::Vec4(_) => DataType::Vec4,
        }
    }
}

// ---------------------------------------------------------------------------
// Node kinds
// ---------------------------------------------------------------------------

/// Every type of node the editor supports.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    // --- Output ---
    Output,

    // --- Constants ---
    FloatConst,
    Vec2Const,
    Vec3Const,
    ColorConst,

    // --- Kroma uniforms ---
    Time,
    DeltaTime,
    Frame,
    Resolution,
    Mouse,
    CpuUsage,
    RamUsage,
    Battery,
    AudioLevel,
    UV,

    // --- Math ---
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Sqrt,
    Abs,
    Negate,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Exp2,
    Log,
    Log2,
    Sign,
    Ceil,
    Round,
    Fract,
    Floor,
    Mod,
    Clamp,
    Mix,
    Step,
    SmoothStep,
    Min,
    Max,
    Saturate,
    OneMinus,
    InverseSqrt,

    // --- Vector ops ---
    Combine2,
    Combine3,
    Combine4,
    SplitVec2,
    SplitVec3,
    SplitVec4,
    Length,
    Normalize,
    Dot,
    Cross,
    Distance,
    Reflect,
    Refract,

    // --- Colour ---
    RgbToHsv,
    HsvToRgb,

    // --- Procedural ---
    ValueNoise,
    Voronoi,

    // --- Mixed-type constructors ---
    Combine4FromVec3Float,      // vec4(vec3, float)
    Combine4FromVec2Vec2,       // vec4(vec2, vec2)
    Combine4FromVec2FloatFloat, // vec4(vec2, float, float)
    Combine3FromVec2Float,      // vec3(vec2, float)

    // --- Vec2 math ---
    AddVec2,
    SubtractVec2,
    MultiplyVec2,
    MultiplyVec2Scalar,
    DivideVec2,
    DivideVec2Scalar,

    // --- Vec3 math ---
    AddVec3,
    SubtractVec3,
    MultiplyVec3,
    MultiplyVec3Scalar,
    DivideVec3,
    DivideVec3Scalar,

    // --- Vec4 math ---
    AddVec4,
    SubtractVec4,
    MultiplyVec4,
    MultiplyVec4Scalar,
    DivideVec4,
    DivideVec4Scalar,

    // --- Custom GLSL ---
    GlslExpr,

    // --- Comparison ---
    LessThan,
    GreaterThan,
    LessEqual,
    GreaterEqual,
    Equal,
    NotEqual,

    // --- Logical ---
    LogicalAnd,
    LogicalOr,
    LogicalNot,

    // --- Bitwise ---
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    LeftShift,
    RightShift,

    // --- Matrix constructors ---
    Mat2,
    Mat3,
    Mat4,

    // --- Control flow ---
    ForLoop,
    Conditional,

    // --- Sampling ---
    TextureSample,

    // --- Custom function call ---
    CustomFunc,
}

impl NodeKind {
    /// Human-readable label for the node palette.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::FloatConst => "Float",
            Self::Vec2Const => "Vec2",
            Self::Vec3Const => "Vec3",
            Self::ColorConst => "Color",
            Self::Time => "Time",
            Self::DeltaTime => "Delta Time",
            Self::Frame => "Frame",
            Self::Resolution => "Resolution",
            Self::Mouse => "Mouse",
            Self::CpuUsage => "CPU Usage",
            Self::RamUsage => "RAM Usage",
            Self::Battery => "Battery",
            Self::AudioLevel => "Audio Level",
            Self::UV => "UV Coords",
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Multiply => "Multiply",
            Self::Divide => "Divide",
            Self::Power => "Power",
            Self::Sqrt => "Square Root",
            Self::Abs => "Absolute",
            Self::Negate => "Negate",
            Self::Sin => "Sine",
            Self::Cos => "Cosine",
            Self::Tan => "Tangent",
            Self::Asin => "Arc Sine",
            Self::Acos => "Arc Cosine",
            Self::Atan => "Arc Tangent",
            Self::Atan2 => "Arc Tangent 2",
            Self::Exp => "Exp",
            Self::Exp2 => "Exp2",
            Self::Log => "Log",
            Self::Log2 => "Log2",
            Self::Sign => "Sign",
            Self::Ceil => "Ceil",
            Self::Round => "Round",
            Self::Fract => "Fract",
            Self::Floor => "Floor",
            Self::Mod => "Modulo",
            Self::Clamp => "Clamp",
            Self::Mix => "Mix / Lerp",
            Self::Step => "Step",
            Self::SmoothStep => "Smooth Step",
            Self::Min => "Min",
            Self::Max => "Max",
            Self::Saturate => "Saturate",
            Self::OneMinus => "One Minus",
            Self::InverseSqrt => "Inverse Sqrt",
            Self::Combine2 => "Combine Vec2",
            Self::Combine3 => "Combine Vec3",
            Self::Combine4 => "Combine Vec4",
            Self::SplitVec2 => "Split Vec2",
            Self::SplitVec3 => "Split Vec3",
            Self::SplitVec4 => "Split Vec4",
            Self::Length => "Length",
            Self::Normalize => "Normalize",
            Self::Dot => "Dot Product",
            Self::Cross => "Cross Product",
            Self::Distance => "Distance",
            Self::Reflect => "Reflect",
            Self::Refract => "Refract",
            Self::RgbToHsv => "RGB → HSV",
            Self::HsvToRgb => "HSV → RGB",
            Self::ValueNoise => "Value Noise",
            Self::Voronoi => "Voronoi",
            Self::GlslExpr => "GLSL Expression",
            Self::LessThan => "Less Than",
            Self::GreaterThan => "Greater Than",
            Self::LessEqual => "Less/Equal",
            Self::GreaterEqual => "Greater/Equal",
            Self::Equal => "Equal",
            Self::NotEqual => "Not Equal",
            Self::LogicalAnd => "AND",
            Self::LogicalOr => "OR",
            Self::LogicalNot => "NOT",
            Self::BitAnd => "Bit AND",
            Self::BitOr => "Bit OR",
            Self::BitXor => "Bit XOR",
            Self::BitNot => "Bit NOT",
            Self::LeftShift => "Left Shift",
            Self::RightShift => "Right Shift",
            Self::Mat2 => "Mat2",
            Self::Mat3 => "Mat3",
            Self::Mat4 => "Mat4",
            Self::ForLoop => "For Loop",
            Self::Conditional => "Conditional",
            Self::TextureSample => "Texture Sample",
            Self::CustomFunc => "Custom Function",

            // Mixed-type constructors
            Self::Combine4FromVec3Float => "Combine Vec4 (Vec3+F)",
            Self::Combine4FromVec2Vec2 => "Combine Vec4 (Vec2+Vec2)",
            Self::Combine4FromVec2FloatFloat => "Combine Vec4 (Vec2+F+F)",
            Self::Combine3FromVec2Float => "Combine Vec3 (Vec2+F)",

            // Vec2 math
            Self::AddVec2 => "Add Vec2",
            Self::SubtractVec2 => "Subtract Vec2",
            Self::MultiplyVec2 => "Multiply Vec2",
            Self::MultiplyVec2Scalar => "Multiply Vec2×Scalar",
            Self::DivideVec2 => "Divide Vec2",
            Self::DivideVec2Scalar => "Divide Vec2÷Scalar",

            // Vec3 math
            Self::AddVec3 => "Add Vec3",
            Self::SubtractVec3 => "Subtract Vec3",
            Self::MultiplyVec3 => "Multiply Vec3",
            Self::MultiplyVec3Scalar => "Multiply Vec3×Scalar",
            Self::DivideVec3 => "Divide Vec3",
            Self::DivideVec3Scalar => "Divide Vec3÷Scalar",

            // Vec4 math
            Self::AddVec4 => "Add Vec4",
            Self::SubtractVec4 => "Subtract Vec4",
            Self::MultiplyVec4 => "Multiply Vec4",
            Self::MultiplyVec4Scalar => "Multiply Vec4×Scalar",
            Self::DivideVec4 => "Divide Vec4",
            Self::DivideVec4Scalar => "Divide Vec4÷Scalar",
        }
    }

    /// Palette category.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::FloatConst | Self::Vec2Const | Self::Vec3Const | Self::ColorConst => "Constants",
            Self::Time
            | Self::DeltaTime
            | Self::Frame
            | Self::Resolution
            | Self::Mouse
            | Self::UV => "Input",
            Self::CpuUsage | Self::RamUsage | Self::Battery | Self::AudioLevel => "System",
            Self::Add
            | Self::Subtract
            | Self::Multiply
            | Self::Divide
            | Self::Power
            | Self::Sqrt
            | Self::Abs
            | Self::Negate
            | Self::Sin
            | Self::Cos
            | Self::Tan
            | Self::Asin
            | Self::Acos
            | Self::Atan
            | Self::Atan2
            | Self::Exp
            | Self::Exp2
            | Self::Log
            | Self::Log2
            | Self::Sign
            | Self::Ceil
            | Self::Round
            | Self::Fract
            | Self::Floor
            | Self::Mod
            | Self::Clamp
            | Self::Mix
            | Self::Step
            | Self::SmoothStep
            | Self::Min
            | Self::Max
            | Self::Saturate
            | Self::OneMinus
            | Self::InverseSqrt
            // Vec2 math
            | Self::AddVec2
            | Self::SubtractVec2
            | Self::MultiplyVec2
            | Self::MultiplyVec2Scalar
            | Self::DivideVec2
            | Self::DivideVec2Scalar
            // Vec3 math
            | Self::AddVec3
            | Self::SubtractVec3
            | Self::MultiplyVec3
            | Self::MultiplyVec3Scalar
            | Self::DivideVec3
            | Self::DivideVec3Scalar
            // Vec4 math
            | Self::AddVec4
            | Self::SubtractVec4
            | Self::MultiplyVec4
            | Self::MultiplyVec4Scalar
            | Self::DivideVec4
            | Self::DivideVec4Scalar => "Math",
            Self::Combine2
            | Self::Combine3
            | Self::Combine4
            | Self::SplitVec2
            | Self::SplitVec3
            | Self::SplitVec4
            | Self::Length
            | Self::Normalize
            | Self::Dot
            | Self::Cross
            | Self::Distance
            | Self::Reflect
            | Self::Refract
            | Self::Combine4FromVec3Float
            | Self::Combine4FromVec2Vec2
            | Self::Combine4FromVec2FloatFloat
            | Self::Combine3FromVec2Float => "Vector",
            Self::RgbToHsv | Self::HsvToRgb => "Color",
            Self::ValueNoise | Self::Voronoi => "Procedural",
            Self::LessThan
            | Self::GreaterThan
            | Self::LessEqual
            | Self::GreaterEqual
            | Self::Equal
            | Self::NotEqual
            | Self::LogicalAnd
            | Self::LogicalOr
            | Self::LogicalNot
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor
            | Self::BitNot
            | Self::LeftShift
            | Self::RightShift => "Logic",
            Self::Mat2 | Self::Mat3 | Self::Mat4 => "Matrix",
            Self::GlslExpr
            | Self::ForLoop
            | Self::Conditional
            | Self::TextureSample
            | Self::CustomFunc => "Custom",
        }
    }

    pub fn inputs(&self) -> &'static [PortDef] {
        crate::nodes::inputs(self)
    }

    pub fn outputs(&self) -> &'static [PortDef] {
        crate::nodes::outputs(self)
    }

    pub fn default_values(&self) -> Vec<DefaultValue> {
        crate::nodes::default_values(self)
    }

    /// Generate the GLSL expression for this node.
    pub fn codegen(
        &self,
        inputs: &[String],
        var_name: &str,
        defaults: &[DefaultValue],
        meta: &Option<String>,
    ) -> String {
        crate::nodes::codegen(self, inputs, var_name, defaults, meta)
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// A placed node in the graph.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Canvas position (top-left corner).
    pub position: [f32; 2],
    /// Default values for each input port.
    pub defaults: Vec<DefaultValue>,
    /// Whether the node is currently selected.
    pub selected: bool,
    /// Optional metadata string.
    ///
    /// - `GlslExpr`: the GLSL expression text (e.g. `"a < b"`)
    /// - `CustomFunc`: the function name (e.g. `"myHelper"`)
    /// - Other node kinds: `None`
    pub meta: Option<String>,
    /// Optional variable name from GLSL source. Used during codegen to
    /// preserve original names in the glsl→nodes→glsl roundtrip.
    pub label: Option<String>,
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind, position: [f32; 2]) -> Self {
        let defaults = kind.default_values();
        Self {
            id,
            kind,
            position,
            defaults,
            selected: false,
            meta: None,
            label: None,
        }
    }

    pub fn inputs(&self) -> &[PortDef] {
        self.kind.inputs()
    }

    pub fn outputs(&self) -> &[PortDef] {
        self.kind.outputs()
    }
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/// A wire between two ports.
#[derive(Debug, Clone, Copy)]
pub struct Connection {
    pub id: ConnectionId,
    pub from: PortAddr,
    pub to: PortAddr,
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Ordered list of all node kinds, grouped by category.
pub fn palette() -> Vec<(&'static str, Vec<NodeKind>)> {
    vec![
        (
            "Input",
            vec![
                NodeKind::UV,
                NodeKind::Time,
                NodeKind::DeltaTime,
                NodeKind::Frame,
                NodeKind::Resolution,
                NodeKind::Mouse,
            ],
        ),
        (
            "System",
            vec![
                NodeKind::CpuUsage,
                NodeKind::RamUsage,
                NodeKind::Battery,
                NodeKind::AudioLevel,
            ],
        ),
        (
            "Constants",
            vec![
                NodeKind::FloatConst,
                NodeKind::Vec2Const,
                NodeKind::Vec3Const,
                NodeKind::ColorConst,
            ],
        ),
        (
            "Math",
            vec![
                NodeKind::Add,
                NodeKind::Subtract,
                NodeKind::Multiply,
                NodeKind::Divide,
                NodeKind::Mod,
                NodeKind::Power,
                NodeKind::Sqrt,
                NodeKind::InverseSqrt,
                NodeKind::Abs,
                NodeKind::Negate,
                NodeKind::Sign,
                NodeKind::Sin,
                NodeKind::Cos,
                NodeKind::Tan,
                NodeKind::Asin,
                NodeKind::Acos,
                NodeKind::Atan,
                NodeKind::Atan2,
                NodeKind::Exp,
                NodeKind::Exp2,
                NodeKind::Log,
                NodeKind::Log2,
                NodeKind::Fract,
                NodeKind::Floor,
                NodeKind::Ceil,
                NodeKind::Round,
                NodeKind::Clamp,
                NodeKind::Mix,
                NodeKind::Step,
                NodeKind::SmoothStep,
                NodeKind::Min,
                NodeKind::Max,
                NodeKind::Saturate,
                NodeKind::OneMinus,
                // Vec2 math
                NodeKind::AddVec2,
                NodeKind::SubtractVec2,
                NodeKind::MultiplyVec2,
                NodeKind::MultiplyVec2Scalar,
                NodeKind::DivideVec2,
                NodeKind::DivideVec2Scalar,
                // Vec3 math
                NodeKind::AddVec3,
                NodeKind::SubtractVec3,
                NodeKind::MultiplyVec3,
                NodeKind::MultiplyVec3Scalar,
                NodeKind::DivideVec3,
                NodeKind::DivideVec3Scalar,
                // Vec4 math
                NodeKind::AddVec4,
                NodeKind::SubtractVec4,
                NodeKind::MultiplyVec4,
                NodeKind::MultiplyVec4Scalar,
                NodeKind::DivideVec4,
                NodeKind::DivideVec4Scalar,
            ],
        ),
        (
            "Vector",
            vec![
                NodeKind::Combine2,
                NodeKind::Combine3,
                NodeKind::Combine4,
                NodeKind::SplitVec2,
                NodeKind::SplitVec3,
                NodeKind::SplitVec4,
                NodeKind::Length,
                NodeKind::Distance,
                NodeKind::Normalize,
                NodeKind::Dot,
                NodeKind::Cross,
                NodeKind::Reflect,
                NodeKind::Refract,
                // Mixed-type constructors
                NodeKind::Combine4FromVec3Float,
                NodeKind::Combine4FromVec2Vec2,
                NodeKind::Combine4FromVec2FloatFloat,
                NodeKind::Combine3FromVec2Float,
            ],
        ),
        ("Color", vec![NodeKind::RgbToHsv, NodeKind::HsvToRgb]),
        ("Procedural", vec![NodeKind::ValueNoise, NodeKind::Voronoi]),
        (
            "Logic",
            vec![
                NodeKind::LessThan,
                NodeKind::GreaterThan,
                NodeKind::LessEqual,
                NodeKind::GreaterEqual,
                NodeKind::Equal,
                NodeKind::NotEqual,
                NodeKind::LogicalAnd,
                NodeKind::LogicalOr,
                NodeKind::LogicalNot,
                NodeKind::BitAnd,
                NodeKind::BitOr,
                NodeKind::BitXor,
                NodeKind::BitNot,
                NodeKind::LeftShift,
                NodeKind::RightShift,
            ],
        ),
        (
            "Matrix",
            vec![NodeKind::Mat2, NodeKind::Mat3, NodeKind::Mat4],
        ),
        (
            "Custom",
            vec![
                NodeKind::GlslExpr,
                NodeKind::ForLoop,
                NodeKind::Conditional,
                NodeKind::TextureSample,
                NodeKind::CustomFunc,
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_value_float_to_glsl() {
        let v = DefaultValue::Float(1.5);
        assert_eq!(v.to_glsl(), "1.500000");
    }

    #[test]
    fn default_value_vec2_to_glsl() {
        let v = DefaultValue::Vec2([1.0, 2.0]);
        assert_eq!(v.to_glsl(), "vec2(1.000000, 2.000000)");
    }

    #[test]
    fn default_value_vec3_to_glsl() {
        let v = DefaultValue::Vec3([1.0, 2.0, 3.0]);
        let glsl = v.to_glsl();
        assert!(glsl.starts_with("vec3("));
        assert!(glsl.contains("3.000000"));
    }

    #[test]
    fn default_value_vec4_to_glsl() {
        let v = DefaultValue::Vec4([0.1, 0.2, 0.3, 1.0]);
        let glsl = v.to_glsl();
        assert!(glsl.starts_with("vec4("));
        assert!(glsl.contains("1.000000"));
    }

    #[test]
    fn default_value_data_type_matches() {
        assert_eq!(DefaultValue::Float(0.0).data_type(), DataType::Float);
        assert_eq!(DefaultValue::Vec2([0.0; 2]).data_type(), DataType::Vec2);
        assert_eq!(DefaultValue::Vec3([0.0; 3]).data_type(), DataType::Vec3);
        assert_eq!(DefaultValue::Vec4([0.0; 4]).data_type(), DataType::Vec4);
    }

    #[test]
    fn data_type_glsl_type_names() {
        assert_eq!(DataType::Float.glsl_type(), "float");
        assert_eq!(DataType::Vec2.glsl_type(), "vec2");
        assert_eq!(DataType::Vec3.glsl_type(), "vec3");
        assert_eq!(DataType::Vec4.glsl_type(), "vec4");
    }
}
