// Shadertoy-compatible example for testing the translator.
// This shader should be importable via `kroma-gui import`.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Simple plasma effect
    float t = iTime;
    float v1 = sin(uv.x * 10.0 + t);
    float v2 = sin(uv.y * 10.0 + t * 1.3);
    float v3 = sin((uv.x + uv.y) * 10.0 + t * 0.7);
    float v4 = sin(length(uv - 0.5) * 20.0 - t * 2.0);

    float v = (v1 + v2 + v3 + v4) / 4.0;

    vec3 col;
    col.r = sin(v * 3.14159) * 0.5 + 0.5;
    col.g = sin(v * 3.14159 + 2.094) * 0.5 + 0.5;
    col.b = sin(v * 3.14159 + 4.189) * 0.5 + 0.5;

    // Mouse interaction
    vec2 mouse = iMouse.xy / iResolution.xy;
    float d = distance(uv, mouse);
    col += smoothstep(0.2, 0.0, d) * 0.3;

    fragColor = vec4(col, 1.0);
}
