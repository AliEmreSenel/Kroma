// Test shader: ALL uniforms combined
// A comprehensive dashboard that visualizes every Kroma uniform:
//   u_time, u_delta_time, u_frame, u_resolution, u_mouse,
//   u_cpu, u_ram, u_battery, u_audio_level
//
// The screen is divided into labeled sections, each showing one uniform.
// All system uniforms (u_cpu, u_ram, u_battery, u_audio_level) are read
// directly from the Globals block — no faked values.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    vec3 col = vec3(0.02, 0.02, 0.04);

    // Grid layout: 3 columns x 3 rows
    float cellX = floor(uv.x * 3.0);
    float cellY = floor(uv.y * 3.0);
    vec2 cellUV = fract(vec2(uv.x * 3.0, uv.y * 3.0));

    // Grid lines
    float gridLine = step(0.98, fract(uv.x * 3.0)) + step(0.98, fract(uv.y * 3.0));
    gridLine = clamp(gridLine, 0.0, 1.0);

    // Cell index: row-major (0=bottom-left, 8=top-right)
    int cell = int(cellY) * 3 + int(cellX);

    // === Cell 0 (bottom-left): u_time — Color cycle ===
    if (cell == 0) {
        float t = iTime;
        float r = 0.5 + 0.5 * sin(t);
        float g = 0.5 + 0.5 * sin(t * 0.7 + 2.094);
        float b = 0.5 + 0.5 * sin(t * 1.3 + 4.189);
        float d = length(cellUV - 0.5);
        float pulse = smoothstep(0.4, 0.0, d);
        col = vec3(r, g, b) * pulse;
    }

    // === Cell 1 (bottom-center): u_delta_time — Smoothness ring ===
    else if (cell == 1) {
        vec2 c = cellUV - 0.5;
        float d = length(c);
        float ring = smoothstep(0.02, 0.0, abs(d - 0.3));
        float speed = sin(iTime * 30.0) * 0.5 + 0.5;
        col = mix(vec3(1.0, 0.3, 0.1), vec3(0.1, 1.0, 0.3), speed) * ring;
    }

    // === Cell 2 (bottom-right): u_frame — Scrolling frame counter ===
    else if (cell == 2) {
        float stripe = 0.5 + 0.5 * sin(cellUV.x * 30.0 - iTime * 12.0);
        float hue = fract(iTime * 0.05);
        float r = abs(hue * 6.0 - 3.0) - 1.0;
        float g = 2.0 - abs(hue * 6.0 - 2.0);
        float b = 2.0 - abs(hue * 6.0 - 4.0);
        col = clamp(vec3(r, g, b), 0.0, 1.0) * stripe;
    }

    // === Cell 3 (middle-left): u_resolution — Adaptive grid ===
    else if (cell == 3) {
        float gx = step(0.95, fract(cellUV.x * iResolution.x / 200.0));
        float gy = step(0.95, fract(cellUV.y * iResolution.y / 200.0));
        float grid = max(gx, gy);
        float rn = iResolution.x / 3840.0;
        float gn = iResolution.y / 2160.0;
        col = mix(vec3(rn * 0.4, gn * 0.4, 0.15), vec3(0.4, 0.6, 0.9), grid);
    }

    // === Cell 4 (middle-center): u_mouse — Spotlight ===
    else if (cell == 4) {
        vec2 mouse = iMouse.xy;
        vec2 localMouse = (mouse * 3.0) - vec2(1.0, 1.0);
        localMouse = clamp(localMouse, 0.0, 1.0);
        float d = length(cellUV - localMouse);
        float spot = smoothstep(0.25, 0.0, d);
        float cross = smoothstep(0.005, 0.0, min(abs(cellUV.x - localMouse.x), abs(cellUV.y - localMouse.y)));
        col = vec3(0.2, 0.8, 1.0) * spot + vec3(0.6, 0.2, 0.2) * cross * 0.3;
    }

    // === Cell 5 (middle-right): u_cpu — Real CPU bar ===
    else if (cell == 5) {
        float cpu = u_cpu;
        float bar = step(cellUV.y, cpu) * step(0.15, cellUV.x) * step(cellUV.x, 0.85);
        vec3 barCol = mix(vec3(0.1, 0.8, 0.2), vec3(1.0, 0.2, 0.1), cpu);
        col = barCol * bar + vec3(0.06) * step(0.15, cellUV.x) * step(cellUV.x, 0.85) * (1.0 - bar);
        // Tick marks
        float ticks = step(0.96, fract(cellUV.y * 10.0)) * step(0.15, cellUV.x) * step(cellUV.x, 0.85);
        col += vec3(0.15) * ticks;
    }

    // === Cell 6 (top-left): u_ram — Real RAM liquid gauge ===
    else if (cell == 6) {
        float ram = u_ram;
        float d = length(cellUV - 0.5);
        float inside = step(d, 0.35);
        float liquidY = 0.15 + ram * 0.7 + 0.005 * sin(cellUV.x * 20.0 + iTime * 4.0);
        float liquid = inside * step(cellUV.y, liquidY);
        vec3 liqCol = mix(vec3(0.1, 0.5, 0.9), vec3(0.9, 0.2, 0.1), ram);
        float ring = smoothstep(0.02, 0.0, abs(d - 0.35));
        col = liqCol * liquid + vec3(0.3, 0.5, 0.7) * ring + vec3(0.05) * inside * (1.0 - liquid);
    }

    // === Cell 7 (top-center): u_battery — Real battery icon ===
    else if (cell == 7) {
        float batt = clamp(u_battery, 0.0, 1.0);
        float noBatt = step(u_battery, -0.01); // 1.0 if no battery
        float inBody = step(0.1, cellUV.x) * step(cellUV.x, 0.85) * step(0.3, cellUV.y) * step(cellUV.y, 0.7);
        float inTerm = step(0.85, cellUV.x) * step(cellUV.x, 0.9) * step(0.4, cellUV.y) * step(cellUV.y, 0.6);
        float fill = step(0.13, cellUV.x) * step(cellUV.x, 0.13 + batt * 0.69) * step(0.33, cellUV.y) * step(cellUV.y, 0.67);
        vec3 battCol;
        if (noBatt > 0.5) {
            battCol = vec3(0.3, 0.3, 0.3); // grey when no battery
        } else if (batt < 0.2) {
            battCol = vec3(1.0, 0.1, 0.1);
        } else if (batt < 0.5) {
            battCol = vec3(1.0, 0.7, 0.0);
        } else {
            battCol = vec3(0.1, 0.9, 0.2);
        }
        float pulse = (batt < 0.2 && noBatt < 0.5) ? 0.6 + 0.4 * sin(iTime * 4.0) : 1.0;
        col = vec3(0.25) * (inBody + inTerm) * (1.0 - fill) + battCol * fill * pulse;
    }

    // === Cell 8 (top-right): u_audio_level — Real audio equalizer ===
    else if (cell == 8) {
        float audio = u_audio_level;
        float numBars = 8.0;
        float barIdx = floor(cellUV.x * numBars);
        float barUVx = fract(cellUV.x * numBars);
        float phase = barIdx * 0.5 + sin(barIdx * 1.7) * 2.0;
        float barH = audio * (0.5 + 0.5 * sin(iTime * 3.0 + phase));
        barH = clamp(barH, 0.02, 0.98);
        float barFill = step(0.15, barUVx) * step(barUVx, 0.85);
        float inBar = barFill * step(cellUV.y, barH);
        vec3 barCol;
        if (cellUV.y < 0.3) {
            barCol = vec3(0.0, 0.8, 0.3);
        } else if (cellUV.y < 0.7) {
            barCol = vec3(1.0, 1.0, 0.0);
        } else {
            barCol = vec3(1.0, 0.1, 0.0);
        }
        col = barCol * 0.8 * inBar;
    }

    // Grid overlay
    col = mix(col, vec3(0.2, 0.25, 0.3), gridLine * 0.6);

    fragColor = vec4(col, 1.0);
}
