// Match the existing software scaler's limited-range conversion and its
// separate signed fixed-point rounding of luma and chroma contributions.
// Write encoded RGBA through the non-sRGB view; shaders sample its sRGB view.
@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(positions[index], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let raw_y = i32(round(textureLoad(luma, pixel, 0).r * 255.0));
    let uv = (vec2<i32>(round(textureLoad(chroma, pixel / 2, 0).rg * 255.0)) - vec2(128)) * 8;
    let y = ((raw_y - 16) * 8 * 9539) >> 16;
    let rgb = clamp(vec3(y + ((uv.y * 13075) >> 16),
        y + ((uv.x * -3209) >> 16) + ((uv.y * -6660) >> 16),
        y + ((uv.x * 16525) >> 16)), vec3(0), vec3(255));
    return vec4(vec3<f32>(rgb) / 255.0, 1.0);
}
