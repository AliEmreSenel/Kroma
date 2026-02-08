// =========================================
// Kroma Example Shader: Gradient Wave
// =========================================
#version 450

uniform float u_time;
uniform float u_delta_time;
uniform int   u_frame;
uniform vec3  u_resolution;
uniform vec4  u_mouse;

// System data uniforms
uniform float u_cpu;
uniform float u_ram;
uniform float u_battery;

void main() {
    vec2 uv = gl_FragCoord.xy / u_resolution.xy;

    // Animated gradient that reacts to time
    float r = 0.5 + 0.5 * sin(u_time * 0.7 + uv.x * 6.2831);
    float g = 0.5 + 0.5 * sin(u_time * 1.1 + uv.y * 6.2831);
    float b = 0.5 + 0.5 * cos(u_time * 0.5 + (uv.x + uv.y) * 3.1416);

    // Subtle reaction to CPU load (brightness boost)
    float cpu_boost = u_cpu * 0.3;
    r = clamp(r + cpu_boost, 0.0, 1.0);

    // Mouse interaction — radial glow at cursor position
    vec2 mouse_norm = u_mouse.xy;
    float dist = distance(uv, mouse_norm);
    float glow = smoothstep(0.3, 0.0, dist) * 0.4;

    gl_FragColor = vec4(r + glow, g + glow, b + glow, 1.0);
}
