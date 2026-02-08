// Test shader: u_time uniform
// Should show smoothly animating colors that change over time.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    float r = 0.5 + 0.5 * sin(iTime);
    float g = 0.5 + 0.5 * sin(iTime * 0.7 + 2.094);
    float b = 0.5 + 0.5 * sin(iTime * 1.3 + 4.189);

    // Radial gradient modulated by time
    float d = length(uv - 0.5);
    float pulse = 0.5 + 0.5 * sin(iTime * 2.0 - d * 10.0);

    fragColor = vec4(r * pulse, g * pulse, b * pulse, 1.0);
}
