// Test shader: u_resolution uniform
// Should show a grid whose density adapts to the actual screen resolution.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Display resolution-dependent grid
    float gridX = step(0.98, fract(uv.x * iResolution.x / 100.0));
    float gridY = step(0.98, fract(uv.y * iResolution.y / 100.0));
    float grid = max(gridX, gridY);

    // Show resolution values as colour channels
    float rNorm = iResolution.x / 3840.0; // Red = width normalised to 4K
    float gNorm = iResolution.y / 2160.0; // Green = height normalised to 4K
    float aspect = iResolution.x / iResolution.y;

    vec3 bg = vec3(rNorm * 0.3, gNorm * 0.3, 0.1);
    vec3 gridColor = vec3(0.4, 0.6, 0.8);
    vec3 col = mix(bg, gridColor, grid);

    // Bottom bar showing aspect ratio
    if (uv.y < 0.05) {
        col = vec3(aspect / 3.0, 0.2, 0.5);
    }

    fragColor = vec4(col, 1.0);
}
