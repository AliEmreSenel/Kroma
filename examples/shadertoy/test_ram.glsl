// Test shader: u_ram uniform visualization
// Visualizes real RAM usage as a liquid fill gauge.
// After translation, u_ram is available directly (0.0 = empty, 1.0 = full).

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // u_ram is declared in the Globals block added by the translator
    float ram = u_ram;

    // Circular gauge
    vec2 center = vec2(0.5, 0.5);
    float radius = 0.35;
    float d = length(uv - center);

    // Ring outline
    float ring = smoothstep(0.02, 0.0, abs(d - radius));

    // Inside the circle
    float inside = step(d, radius - 0.02);

    // Liquid level with wave animation
    float liquidY = center.y - radius + 0.02 + ram * (2.0 * radius - 0.04);
    float wave = liquidY + 0.008 * sin(uv.x * 25.0 + iTime * 4.0);
    float liquid = inside * step(uv.y, wave);

    // Color: blue when low, red when high
    vec3 liquidCol = mix(vec3(0.1, 0.5, 0.9), vec3(0.9, 0.2, 0.1), ram);
    vec3 bgCol = vec3(0.05, 0.05, 0.08);
    vec3 ringCol = vec3(0.3, 0.5, 0.7);
    vec3 emptyCol = vec3(0.08, 0.08, 0.12);

    vec3 col = bgCol;
    col = mix(col, emptyCol, inside * (1.0 - liquid));
    col = mix(col, liquidCol, liquid);
    col = mix(col, ringCol, ring);

    // RAM percentage bar at bottom
    if (uv.y < 0.06) {
        float bar = step(uv.x, ram);
        col = mix(vec3(0.08), liquidCol, bar);
    }

    fragColor = vec4(col, 1.0);
}
