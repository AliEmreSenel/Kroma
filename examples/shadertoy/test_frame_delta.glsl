// Test shader: u_frame and u_delta_time uniform visualization
// Shows frame counter as scrolling digits and delta time as a smoothness meter.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Background
    vec3 col = vec3(0.03, 0.03, 0.06);

    // --- Top half: Frame counter visualization ---
    if (uv.y > 0.5) {
        vec2 topUV = vec2(uv.x, (uv.y - 0.5) * 2.0);

        // Scrolling stripes — each stripe is one frame
        // iFrame increments every frame, mapped to hue cycle
        float framePhase = iTime * 60.0; // approximate frame count from time
        float stripe = sin(topUV.x * 50.0 - framePhase * 0.5);
        stripe = 0.5 + 0.5 * stripe;

        // Color cycles with time (hue rotation)
        float hue = fract(iTime * 0.1);
        // Simple HSV to RGB
        float r = abs(hue * 6.0 - 3.0) - 1.0;
        float g = 2.0 - abs(hue * 6.0 - 2.0);
        float b = 2.0 - abs(hue * 6.0 - 4.0);
        vec3 hsvCol = clamp(vec3(r, g, b), 0.0, 1.0);

        col = hsvCol * stripe * 0.8;

        // Speed indicator on the right
        if (topUV.x > 0.9) {
            float speed = fract(iTime * 10.0);
            float bar = step(topUV.y, speed);
            col = mix(vec3(0.1), vec3(0.0, 0.8, 1.0), bar);
        }
    }
    // --- Bottom half: Delta time visualization ---
    else {
        vec2 botUV = vec2(uv.x, uv.y * 2.0);

        // Simulate delta time as a smoothness graph
        // Each column represents a "frame" in the recent history
        float dt = iTimeDelta;
        float targetDt = 1.0 / 60.0; // 60 FPS target

        // Pulsing ring that shows timing consistency
        vec2 center = vec2(0.5, 0.5);
        float d = length(botUV - center);

        // Ring radius oscillates with delta time variations
        float ringR = 0.25 + 0.05 * sin(iTime * 30.0);
        float ring = smoothstep(0.02, 0.0, abs(d - ringR));

        // Color: green = smooth, red = laggy
        float smoothness = 1.0 - clamp(abs(dt - targetDt) / targetDt, 0.0, 1.0);
        vec3 ringColor = mix(vec3(1.0, 0.2, 0.1), vec3(0.1, 1.0, 0.3), smoothness);

        col = ringColor * ring * 0.9;

        // Radial lines showing frame ticks
        float angle = atan(botUV.y - center.y, botUV.x - center.x);
        float ticks = step(0.98, fract(angle * 10.0 / 6.283 + iTime * 2.0));
        col += ringColor * ticks * 0.2 * step(d, ringR + 0.05) * step(ringR - 0.05, d);
    }

    // Divider line
    float divider = smoothstep(0.003, 0.0, abs(uv.y - 0.5));
    col += vec3(0.3) * divider;

    fragColor = vec4(col, 1.0);
}
