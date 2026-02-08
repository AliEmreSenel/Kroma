// Test shader: u_battery uniform visualization
// Visualizes real battery charge as a stylized battery icon.
// After translation, u_battery is available directly (0.0–1.0, negative = no battery).

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    vec2 st = vec2(uv.x * aspect, uv.y);

    // u_battery from Globals block (0.0–1.0, or < 0 if no battery)
    float battery = u_battery;
    float noBattery = step(battery, -0.01); // 1.0 if no battery, 0.0 otherwise
    battery = clamp(battery, 0.0, 1.0);

    // Battery body rectangle (centered)
    float cx = aspect * 0.5;
    float cy = 0.5;
    float bw = 0.5;  // battery width
    float bh = 0.25; // battery height

    float bodyL = cx - bw * 0.5;
    float bodyR = cx + bw * 0.5;
    float bodyB = cy - bh * 0.5;
    float bodyT = cy + bh * 0.5;

    // Battery terminal (small nub on right)
    float termL = bodyR;
    float termR = bodyR + 0.04;
    float termB = cy - 0.05;
    float termT = cy + 0.05;

    // Inside battery body
    float inBody = step(bodyL, st.x) * step(st.x, bodyR) * step(bodyB, st.y) * step(st.y, bodyT);
    float inTerm = step(termL, st.x) * step(st.x, termR) * step(termB, st.y) * step(st.y, termT);

    // Battery outline (slightly larger)
    float margin = 0.01;
    float inOutline = step(bodyL - margin, st.x) * step(st.x, bodyR + margin) *
                      step(bodyB - margin, st.y) * step(st.y, bodyT + margin);

    // Fill level inside battery
    float fillMargin = 0.02;
    float fillL = bodyL + fillMargin;
    float fillR = bodyL + fillMargin + battery * (bw - 2.0 * fillMargin);
    float fillB = bodyB + fillMargin;
    float fillT = bodyT - fillMargin;
    float inFill = step(fillL, st.x) * step(st.x, fillR) * step(fillB, st.y) * step(st.y, fillT);

    // Segment lines every 25%
    float seg1 = step(abs(st.x - (bodyL + bw * 0.25)), 0.002) * inBody;
    float seg2 = step(abs(st.x - (bodyL + bw * 0.50)), 0.002) * inBody;
    float seg3 = step(abs(st.x - (bodyL + bw * 0.75)), 0.002) * inBody;
    float segments = max(seg1, max(seg2, seg3));

    // Color based on charge level
    vec3 fillColor;
    if (battery < 0.2) {
        fillColor = vec3(1.0, 0.1, 0.1); // red — critical
    } else if (battery < 0.5) {
        fillColor = vec3(1.0, 0.7, 0.0); // orange — low
    } else {
        fillColor = vec3(0.1, 0.9, 0.2); // green — good
    }

    // Pulsing glow when low
    float pulse = 1.0;
    if (battery < 0.2) {
        pulse = 0.6 + 0.4 * sin(iTime * 4.0);
    }

    // Background
    vec3 bg = vec3(0.03, 0.03, 0.06);
    vec3 outlineCol = vec3(0.5, 0.5, 0.6);
    vec3 bodyCol = vec3(0.1, 0.1, 0.12);

    vec3 col = bg;
    col = mix(col, outlineCol, inOutline);
    col = mix(col, bodyCol, inBody);
    col = mix(col, outlineCol, inTerm);
    col = mix(col, fillColor * pulse, inFill);
    col = mix(col, vec3(0.2), segments * 0.3);

    // Charge percentage text bar at bottom
    if (uv.y < 0.06) {
        float bar = step(uv.x, battery);
        col = mix(vec3(0.05), fillColor * 0.8, bar);
    }

    fragColor = vec4(col, 1.0);
}
