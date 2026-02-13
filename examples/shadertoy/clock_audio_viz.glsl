// Audio Visualizer: Digital clock with FFT-reactive bars
// Shows current time in a stylish digital format with equalizer bars
// emanating outward that react to the audio spectrum.

// -------------------------------------------------------
// Segment-based digital clock rendering
// -------------------------------------------------------

// 7-segment display for a single digit
// Segments: top(0), top-right(1), bottom-right(2), bottom(3),
//           bottom-left(4), top-left(5), middle(6)
float segment(vec2 p, int seg) {
    // Horizontal segments
    if (seg == 0 || seg == 3 || seg == 6) {
        float y = (seg == 0) ? 1.0 : (seg == 6 ? 0.5 : 0.0);
        vec2 c = p - vec2(0.5, y);
        return smoothstep(0.03, 0.01, abs(c.y)) * smoothstep(0.0, 0.05, c.x + 0.35) * smoothstep(0.0, 0.05, 0.35 - c.x);
    }
    // Vertical segments
    float x = (seg == 1 || seg == 2) ? 0.85 : 0.15;
    float yLo = (seg == 1 || seg == 5) ? 0.5 : 0.0;
    float yHi = (seg == 1 || seg == 5) ? 1.0 : 0.5;
    vec2 c = p - vec2(x, (yLo + yHi) * 0.5);
    return smoothstep(0.03, 0.01, abs(c.x)) * smoothstep(0.0, 0.05, c.y - (yLo - (yHi-yLo)*0.5) + (yHi-yLo)*0.5)
         * smoothstep(0.0, 0.05, ((yHi-yLo)*0.5) - c.y + (yLo - (yHi-yLo)*0.5) + (yHi-yLo));
}

// Which segments are active for digit 0-9
float digitSegments(vec2 p, int d) {
    //                           0  1  2  3  4  5  6
    // Encode: bit 0=top, 1=TR, 2=BR, 3=bot, 4=BL, 5=TL, 6=mid
    int segs[10] = int[10](
        0x3F, // 0: all except mid
        0x06, // 1: TR, BR
        0x5B, // 2: top, TR, mid, BL, bot
        0x4F, // 3: top, TR, mid, BR, bot
        0x66, // 4: TL, TR, mid, BR
        0x6D, // 5: top, TL, mid, BR, bot
        0x7D, // 6: top, TL, mid, BR, BL, bot
        0x07, // 7: top, TR, BR
        0x7F, // 8: all
        0x6F  // 9: all except BL
    );

    if (d < 0 || d > 9) return 0.0;
    int mask = segs[d];
    float result = 0.0;
    for (int i = 0; i < 7; i++) {
        if (((mask >> i) & 1) == 1) {
            result += segment(p, i);
        }
    }
    return clamp(result, 0.0, 1.0);
}

// Draw colon separator
float drawColon(vec2 p) {
    float d1 = length(p - vec2(0.5, 0.65));
    float d2 = length(p - vec2(0.5, 0.35));
    return smoothstep(0.06, 0.03, d1) + smoothstep(0.06, 0.03, d2);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    vec3 col = vec3(0.0);

    // -------------------------------------------------------
    // Background: dark gradient with subtle radial vignette
    // -------------------------------------------------------
    float vignette = 1.0 - 0.4 * length(uv - 0.5);
    vec3 bg = vec3(0.02, 0.02, 0.06) * vignette;

    // Animated grid in background
    float grid = 0.0;
    float gridScale = 40.0;
    float gx = abs(sin(uv.x * gridScale + iTime * 0.3));
    float gy = abs(sin(uv.y * gridScale + iTime * 0.2));
    grid = smoothstep(0.98, 1.0, gx) + smoothstep(0.98, 1.0, gy);
    bg += vec3(0.03, 0.05, 0.1) * grid;

    col = bg;

    // -------------------------------------------------------
    // Digital Clock (center of screen, upper half)
    // -------------------------------------------------------
    float totalSeconds = iTime;
    // Use u_time to derive a clock (wraps every 24h for display)
    float daySeconds = mod(totalSeconds, 86400.0);
    int hours = int(daySeconds) / 3600;
    int minutes = (int(daySeconds) % 3600) / 60;
    int seconds = int(daySeconds) % 60;

    // Clock positioning
    float clockY = 0.62;
    float clockScale = 0.09;
    float digitW = clockScale * 1.2;
    float colonW = clockScale * 0.6;
    float totalW = 6.0 * digitW + 2.0 * colonW;
    float clockX = 0.5 - totalW * 0.5;

    float clockGlow = 0.0;
    vec3 clockColor = vec3(0.0, 0.9, 1.0);

    // Blink colon every second
    float colonBlink = step(0.5, fract(iTime));

    // Hours
    float x = clockX;
    vec2 dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, hours / 10);
    x += digitW;
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, hours % 10);
    x += digitW;

    // Colon 1
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += drawColon(dp) * colonBlink;
    x += colonW;

    // Minutes
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, minutes / 10);
    x += digitW;
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, minutes % 10);
    x += digitW;

    // Colon 2
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += drawColon(dp) * colonBlink;
    x += colonW;

    // Seconds
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, seconds / 10);
    x += digitW;
    dp = (uv - vec2(x, clockY)) / clockScale;
    clockGlow += digitSegments(dp, seconds % 10);

    clockGlow = clamp(clockGlow, 0.0, 1.0);

    // Add glow bloom around clock digits
    float bloom = 0.0;
    for (float dy = -2.0; dy <= 2.0; dy += 1.0) {
        for (float dx = -2.0; dx <= 2.0; dx += 1.0) {
            vec2 offset = vec2(dx, dy) / iResolution.xy * 3.0;
            // Simplified bloom: just add soft glow around the text position
            float d = length(vec2(dx, dy));
            bloom += clockGlow * exp(-d * 0.5) * 0.05;
        }
    }
    col += clockColor * (clockGlow + bloom * 0.3);

    // Subtle second-hand progress arc around clock
    float arcAngle = fract(iTime) * 6.28318;
    vec2 arcCenter = vec2(0.5, clockY + clockScale * 0.5);
    vec2 arcP = uv - arcCenter;
    arcP.x *= aspect;
    float arcDist = length(arcP);
    float arcA = atan(arcP.y, arcP.x) + 3.14159;
    float arcR = 0.13;
    float arcWidth = 0.003;
    float arc = smoothstep(arcWidth, 0.0, abs(arcDist - arcR)) * step(arcA, arcAngle);
    col += clockColor * 0.5 * arc;

    // -------------------------------------------------------
    // Audio FFT Bars (bottom half)
    // -------------------------------------------------------
    float barRegionTop = 0.50;
    float barRegionBot = 0.05;

    if (uv.y < barRegionTop && uv.y > barRegionBot) {
        float numBars = 32.0;
        float barIdx = floor(uv.x * numBars);
        float barUV = fract(uv.x * numBars);

        // Simulate per-bar FFT data using u_audio_level and time
        // In a real setup, each bar would read from an audio texture.
        // Here we approximate with varying phases based on bar index.
        float audioBase = u_audio_level;
        float phase1 = sin(barIdx * 0.7 + iTime * 2.5) * 0.5 + 0.5;
        float phase2 = sin(barIdx * 1.3 - iTime * 1.8 + 1.5) * 0.5 + 0.5;
        float phase3 = sin(barIdx * 0.4 + iTime * 3.2 + 3.0) * 0.5 + 0.5;
        float barLevel = audioBase * (0.3 + 0.4 * phase1 + 0.3 * phase2);
        // Add some bass boost for lower bars
        float bassBoost = 1.0 + 2.0 * exp(-barIdx * 0.15);
        barLevel *= bassBoost;
        barLevel = clamp(barLevel, 0.02, 1.0);

        // Map bar height to region
        float barMaxH = barRegionTop - barRegionBot;
        float barH = barRegionBot + barLevel * barMaxH;

        // Bar gap
        float gap = smoothstep(0.08, 0.12, barUV) * smoothstep(0.92, 0.88, barUV);

        float inBar = gap * step(uv.y, barH) * step(barRegionBot, uv.y);

        // Color: hue based on frequency (bar index)
        float hue = barIdx / numBars;
        float audio_hue_shift = iTime * 0.1;
        hue = fract(hue + audio_hue_shift);

        // HSV to RGB
        float h = hue * 6.0;
        float c_val = 0.9;
        float x_val = c_val * (1.0 - abs(mod(h, 2.0) - 1.0));
        vec3 barColor;
        if (h < 1.0) barColor = vec3(c_val, x_val, 0.0);
        else if (h < 2.0) barColor = vec3(x_val, c_val, 0.0);
        else if (h < 3.0) barColor = vec3(0.0, c_val, x_val);
        else if (h < 4.0) barColor = vec3(0.0, x_val, c_val);
        else if (h < 5.0) barColor = vec3(x_val, 0.0, c_val);
        else barColor = vec3(c_val, 0.0, x_val);

        // Brightness varies with height (brighter near top of bar)
        float heightFactor = (uv.y - barRegionBot) / barMaxH;
        barColor *= 0.5 + 0.5 * heightFactor;

        // Glow at bar top
        float topGlow = gap * smoothstep(0.02, 0.0, abs(uv.y - barH)) * 0.8;

        col += barColor * inBar * 0.9;
        col += barColor * topGlow;

        // Reflection below (subtle mirror)
        float reflY = barRegionBot - (uv.y - barRegionBot);
        if (uv.y < barRegionBot) {
            float reflInBar = gap * step(reflY, barH) * 0.15;
            col += barColor * reflInBar;
        }
    }

    // -------------------------------------------------------
    // Circular audio visualizer around clock
    // -------------------------------------------------------
    vec2 center = vec2(0.5, clockY + clockScale * 0.5);
    vec2 p = uv - center;
    p.x *= aspect;
    float dist = length(p);
    float angle = atan(p.y, p.x);

    float ringR = 0.16;
    float ringW = 0.04;

    if (abs(dist - ringR) < ringW * 2.0) {
        // Sample audio at this angle
        float normAngle = (angle + 3.14159) / 6.28318;
        float barPhase = sin(normAngle * 64.0 + iTime * 2.0) * 0.5 + 0.5;
        float barH = u_audio_level * barPhase * ringW * 2.0;

        float ringDist = abs(dist - ringR);
        float inRing = smoothstep(ringW + barH, ringW + barH - 0.003, ringDist);
        float baseRing = smoothstep(0.002, 0.0, abs(dist - ringR));

        // Color based on angle
        float ringHue = fract(normAngle + iTime * 0.05);
        vec3 ringCol = vec3(
            0.5 + 0.5 * cos(6.28318 * (ringHue + 0.0)),
            0.5 + 0.5 * cos(6.28318 * (ringHue + 0.33)),
            0.5 + 0.5 * cos(6.28318 * (ringHue + 0.67))
        );

        col += ringCol * (inRing * 0.4 + baseRing * 0.2) * u_audio_level * 2.0;
    }

    // -------------------------------------------------------
    // Final adjustments
    // -------------------------------------------------------
    // Subtle film grain
    float grain = fract(sin(dot(fragCoord, vec2(12.9898, 78.233)) + iTime) * 43758.5453);
    col += (grain - 0.5) * 0.02;

    // Gamma correction
    col = pow(max(col, 0.0), vec3(0.95));

    fragColor = vec4(col, 1.0);
}
