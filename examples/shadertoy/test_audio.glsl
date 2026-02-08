// Test shader: u_audio_level uniform visualization
// Visualizes real audio input level as equalizer bars.
// After translation, u_audio_level is available directly (0.0 = silent, 1.0 = loud).

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // u_audio_level from Globals block
    float audioLevel = u_audio_level;

    // Number of bars
    float numBars = 16.0;
    float barIdx = floor(uv.x * numBars);
    float barUV = fract(uv.x * numBars); // position within a bar

    // Each bar has slightly different phase for visual interest
    float phase = barIdx * 0.4 + sin(barIdx * 1.7) * 2.0;
    float barHeight = audioLevel * (0.5 + 0.5 * sin(iTime * 3.0 + phase));
    barHeight = clamp(barHeight, 0.02, 0.98);

    // Bar gap
    float barFill = step(0.1, barUV) * step(barUV, 0.9);

    // Is this pixel inside the bar?
    float inBar = barFill * step(uv.y, barHeight);

    // Color gradient: bottom green → mid yellow → top red
    vec3 barColor;
    if (uv.y < 0.3) {
        barColor = mix(vec3(0.0, 0.6, 0.2), vec3(0.0, 1.0, 0.3), uv.y / 0.3);
    } else if (uv.y < 0.7) {
        barColor = mix(vec3(0.0, 1.0, 0.3), vec3(1.0, 1.0, 0.0), (uv.y - 0.3) / 0.4);
    } else {
        barColor = mix(vec3(1.0, 1.0, 0.0), vec3(1.0, 0.1, 0.0), (uv.y - 0.7) / 0.3);
    }

    // Peak indicator (small bright dot at bar height)
    float peakDist = abs(uv.y - barHeight);
    float peak = barFill * smoothstep(0.015, 0.005, peakDist);

    // Glow at bottom
    float glow = barFill * inBar * 0.3 * (1.0 - uv.y);

    // Background with subtle grid
    float gridX = step(0.98, fract(uv.x * numBars));
    float gridY = step(0.98, fract(uv.y * 20.0));
    float grid = max(gridX, gridY) * 0.05;

    vec3 bg = vec3(0.02, 0.02, 0.04) + grid;
    vec3 col = bg;
    col = mix(col, barColor * 0.8, inBar);
    col += barColor * glow;
    col += vec3(1.0) * peak;

    // Master level indicator at the very bottom
    if (uv.y < 0.03) {
        float lvl = smoothstep(0.0, 1.0, audioLevel);
        float bar = step(uv.x, lvl);
        vec3 lvlColor = mix(vec3(0.0, 1.0, 0.3), vec3(1.0, 0.1, 0.0), uv.x);
        col = mix(vec3(0.05), lvlColor, bar);
    }

    fragColor = vec4(col, 1.0);
}
