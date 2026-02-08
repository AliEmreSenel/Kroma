// Test shader: u_cpu uniform
// Visualizes real CPU usage from the Kroma Globals uniform block.
// After translation, u_cpu is available directly (0.0 = idle, 1.0 = 100%).

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // u_cpu is declared in the Globals block added by the translator
    float cpu = u_cpu;

    // Multiple CPU core bars (simulated with different thresholds)
    float numCores = 8.0;
    float coreIdx = floor(uv.x * numCores);
    float coreUV = fract(uv.x * numCores);

    // Each core shows slightly jittered load around the main value
    float jitter = sin(coreIdx * 7.13 + iTime * 0.5) * 0.1;
    float coreLoad = clamp(cpu + jitter, 0.0, 1.0);

    // Bar with gap
    float barFill = step(0.08, coreUV) * step(coreUV, 0.92);
    float inBar = barFill * step(uv.y, coreLoad * 0.85 + 0.05);

    // Green → yellow → red gradient based on load
    vec3 barColor;
    if (coreLoad < 0.5) {
        barColor = mix(vec3(0.1, 0.8, 0.2), vec3(1.0, 1.0, 0.0), coreLoad * 2.0);
    } else {
        barColor = mix(vec3(1.0, 1.0, 0.0), vec3(1.0, 0.15, 0.05), (coreLoad - 0.5) * 2.0);
    }

    // Background
    vec3 bg = vec3(0.04, 0.04, 0.07);
    vec3 col = mix(bg, barColor, inBar);

    // Overall CPU meter at bottom
    if (uv.y < 0.04) {
        float bar = step(uv.x, cpu);
        vec3 meterCol = mix(vec3(0.1, 0.8, 0.3), vec3(1.0, 0.1, 0.05), cpu);
        col = mix(vec3(0.06), meterCol, bar);
    }

    // Subtle grid lines
    float gridY = step(0.97, fract(uv.y * 10.0)) * 0.08;
    col += gridY;

    fragColor = vec4(col, 1.0);
}
