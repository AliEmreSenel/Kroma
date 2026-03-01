struct Globals {
    u_time: f32,
    u_delta_time: f32,
    u_frame: u32,
    _pad0: u32,
    u_resolution: vec2<f32>,
    _pad1: vec2<f32>,
    u_mouse: vec4<f32>,
    u_cpu: f32,
    u_ram: f32,
    u_battery: f32,
    u_audio_level: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

fn message_code(index: i32) -> i32 {
    switch index {
        case 0: {
            return 0; // N
        }
        case 1: {
            return 1; // O
        }
        case 2: {
            return -1; // space
        }
        case 3: {
            return 2; // S
        }
        case 4: {
            return 3; // H
        }
        case 5: {
            return 4; // A
        }
        case 6: {
            return 5; // D
        }
        case 7: {
            return 6; // E
        }
        case 8: {
            return -1; // space
        }
        case 9: {
            return 7; // L
        }
        case 10: {
            return 1; // O
        }
        case 11: {
            return 4; // A
        }
        case 12: {
            return 5; // D
        }
        case 13: {
            return 6; // E
        }
        case 14: {
            return 5; // D
        }
        default: {
            return -1;
        }
    }
}

fn glyph_row(code: i32, row: i32) -> u32 {
    switch code {
        case 0: {
            // N
            switch row {
                case 0: { return 17u; }
                case 1: { return 25u; }
                case 2: { return 21u; }
                case 3: { return 19u; }
                case 4: { return 17u; }
                case 5: { return 17u; }
                case 6: { return 17u; }
                default: { return 0u; }
            }
        }
        case 1: {
            // O
            switch row {
                case 0: { return 14u; }
                case 1: { return 17u; }
                case 2: { return 17u; }
                case 3: { return 17u; }
                case 4: { return 17u; }
                case 5: { return 17u; }
                case 6: { return 14u; }
                default: { return 0u; }
            }
        }
        case 2: {
            // S
            switch row {
                case 0: { return 15u; }
                case 1: { return 16u; }
                case 2: { return 16u; }
                case 3: { return 14u; }
                case 4: { return 1u; }
                case 5: { return 1u; }
                case 6: { return 30u; }
                default: { return 0u; }
            }
        }
        case 3: {
            // H
            switch row {
                case 0: { return 17u; }
                case 1: { return 17u; }
                case 2: { return 17u; }
                case 3: { return 31u; }
                case 4: { return 17u; }
                case 5: { return 17u; }
                case 6: { return 17u; }
                default: { return 0u; }
            }
        }
        case 4: {
            // A
            switch row {
                case 0: { return 14u; }
                case 1: { return 17u; }
                case 2: { return 17u; }
                case 3: { return 31u; }
                case 4: { return 17u; }
                case 5: { return 17u; }
                case 6: { return 17u; }
                default: { return 0u; }
            }
        }
        case 5: {
            // D
            switch row {
                case 0: { return 30u; }
                case 1: { return 17u; }
                case 2: { return 17u; }
                case 3: { return 17u; }
                case 4: { return 17u; }
                case 5: { return 17u; }
                case 6: { return 30u; }
                default: { return 0u; }
            }
        }
        case 6: {
            // E
            switch row {
                case 0: { return 31u; }
                case 1: { return 16u; }
                case 2: { return 16u; }
                case 3: { return 30u; }
                case 4: { return 16u; }
                case 5: { return 16u; }
                case 6: { return 31u; }
                default: { return 0u; }
            }
        }
        case 7: {
            // L
            switch row {
                case 0: { return 16u; }
                case 1: { return 16u; }
                case 2: { return 16u; }
                case 3: { return 16u; }
                case 4: { return 16u; }
                case 5: { return 16u; }
                case 6: { return 31u; }
                default: { return 0u; }
            }
        }
        default: {
            return 0u;
        }
    }
}

fn text_pixel(pixel: vec2<i32>, resolution: vec2<f32>) -> bool {
    let scale = max(2, i32(floor(resolution.y / 220.0)));
    let char_w = 5 * scale;
    let char_h = 7 * scale;
    let spacing = scale;
    let text_len = 15;

    let res_x = max(1, i32(resolution.x));
    let res_y = max(1, i32(resolution.y));

    let text_w = text_len * char_w + (text_len - 1) * spacing;
    let start_x = (res_x - text_w) / 2;
    let start_y = (res_y - char_h) / 2;

    let rel_x = pixel.x - start_x;
    let rel_y = pixel.y - start_y;

    if rel_x < 0 || rel_y < 0 || rel_x >= text_w || rel_y >= char_h {
        return false;
    }

    let cell_w = char_w + spacing;
    let char_index = rel_x / cell_w;
    let within_x = rel_x - (char_index * cell_w);

    if within_x >= char_w {
        return false;
    }

    let glyph = message_code(char_index);
    if glyph < 0 {
        return false;
    }

    let glyph_x = within_x / scale;
    let glyph_y = rel_y / scale;
    let row_mask = glyph_row(glyph, glyph_y);
    let bit = (row_mask >> u32(4 - glyph_x)) & 1u;
    return bit == 1u;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let center = vec2<f32>(0.5, 0.5);
    let dist = distance(uv, center);
    let vignette = 1.0 - smoothstep(0.2, 0.9, dist);

    let stripe = step(0.5, fract((uv.x + uv.y) * 24.0));
    let base = vec3<f32>(0.05, 0.06, 0.08);
    let accent = vec3<f32>(0.18, 0.20, 0.26) * stripe * 0.18;
    var color = base + accent + vec3<f32>(vignette * 0.04);

    let pixel = vec2<i32>(
        i32(floor(uv.x * globals.u_resolution.x)),
        i32(floor(uv.y * globals.u_resolution.y)),
    );

    let shadow_offset = max(1, i32(floor(globals.u_resolution.y / 900.0)));
    let shadow_on = text_pixel(pixel + vec2<i32>(shadow_offset, shadow_offset), globals.u_resolution);
    let text_on = text_pixel(pixel, globals.u_resolution);

    if shadow_on {
        color = mix(color, vec3<f32>(0.0, 0.0, 0.0), 0.35);
    }

    if text_on {
        color = vec3<f32>(0.92, 0.94, 0.98);
    }

    return vec4<f32>(color, 1.0);
}
