// Test shader: u_mouse uniform
// Should show a bright spotlight that follows the cursor position.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Mouse position (normalised 0..1)
    vec2 mouse = iMouse.xy;

    // Distance from cursor
    float d = length(uv - mouse);

    // Spotlight circle
    float spot = smoothstep(0.15, 0.0, d);

    // Crosshair at cursor position
    float crossH = smoothstep(0.003, 0.0, abs(uv.y - mouse.y));
    float crossV = smoothstep(0.003, 0.0, abs(uv.x - mouse.x));
    float cross = max(crossH, crossV) * 0.3;

    vec3 bg = vec3(0.05, 0.05, 0.1);
    vec3 spotColor = vec3(0.2, 0.8, 1.0);
    vec3 crossColor = vec3(0.8, 0.2, 0.2);

    vec3 col = bg + spotColor * spot + crossColor * cross;
    fragColor = vec4(col, 1.0);
}
