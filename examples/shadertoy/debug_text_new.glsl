// Debug shader: Displays ALL Kroma uniforms as readable text on screen.
// Uses a 5×6 bitmap font with smooth anti-aliasing and glow effects.
// Each uniform is displayed with its label, value, and bar gauge.

// -------------------------------------------------------
// 5×6 bitmap font — 30 bits packed per character
// Encoding: bit_idx = iy * 5 + (4 - ix)
//   iy=0 bottom of char, iy=5 top; ix=0 left, ix=4 right
// -------------------------------------------------------

int fontBits(int c) {
    if (c == 32) return 0;           // (space)
    if (c == 37) return 866263411;   // %
    if (c == 45) return 1015808;     // -
    if (c == 46) return 4;           // .
    if (c == 47) return 35791360;    // /
    if (c == 48) return 488162862;   // 0
    if (c == 49) return 146935950;   // 1
    if (c == 50) return 487657759;   // 2
    if (c == 51) return 487787054;   // 3
    if (c == 52) return 73747426;    // 4
    if (c == 53) return 1057949230;  // 5
    if (c == 54) return 487540270;   // 6
    if (c == 55) return 1041305732;  // 7
    if (c == 56) return 488064558;   // 8
    if (c == 57) return 488160302;   // 9
    if (c == 58) return 4194432;     // :
    if (c == 65) return 488177201;   // A
    if (c == 66) return 1025459774;  // B
    if (c == 67) return 488129070;   // C
    if (c == 68) return 1025033790;  // D
    if (c == 69) return 1057964575;  // E
    if (c == 70) return 1057964560;  // F
    if (c == 71) return 488136238;   // G
    if (c == 72) return 589284913;   // H
    if (c == 73) return 474091662;   // I
    if (c == 75) return 590236209;   // K
    if (c == 76) return 554189343;   // L
    if (c == 77) return 599442993;   // M
    if (c == 78) return 597347889;   // N
    if (c == 79) return 488162862;   // O
    if (c == 80) return 1025047056;  // P
    if (c == 82) return 1025047121;  // R
    if (c == 83) return 520553534;   // S
    if (c == 84) return 1044516996;  // T
    if (c == 85) return 588826158;   // U
    if (c == 86) return 588818756;   // V
    if (c == 87) return 588961649;   // W
    if (c == 88) return 581046609;   // X
    if (c == 89) return 581046404;   // Y
    return 0;
}

// Sample a character with smooth anti-aliasing
float charSample(vec2 p, int c) {
    if (p.x < -0.05 || p.y < -0.05 || p.x >= 1.05 || p.y >= 1.05) return 0.0;
    int bits = fontBits(c);
    if (bits == 0 && c != 32) return 0.0;

    // Pixel coordinates (fractional for smoothing)
    float fx = p.x * 5.0;
    float fy = p.y * 6.0;
    int ix = int(floor(fx));
    int iy = int(floor(fy));

    // Clamp to valid range
    if (ix < 0 || ix > 4 || iy < 0 || iy > 5) return 0.0;

    // Get the pixel value at this grid cell
    int idx = iy * 5 + (4 - ix);
    float pixel = float((bits >> idx) & 1);

    // Smooth edges: soften based on distance to pixel center
    float dx = fract(fx) - 0.5;
    float dy = fract(fy) - 0.5;
    float dist = length(vec2(dx, dy));
    float smooth = pixel * smoothstep(0.7, 0.3, dist);

    return smooth;
}

// Print a single character at a position
float printChar(vec2 uv, vec2 pos, float scale, int c) {
    vec2 p = (uv - pos) / vec2(scale, scale * 1.2); // slightly taller than wide
    return charSample(p, c);
}

// Print a float with optional sign and 2 decimals
float printFloat(vec2 uv, vec2 pos, float scale, float value, out float endX) {
    float result = 0.0;
    float x = pos.x;
    float sp = scale * 1.15;
    endX = x;

    bool neg = value < 0.0;
    float v = abs(value);

    if (neg) {
        result += printChar(uv, vec2(x, pos.y), scale, 45);
        x += sp;
    }

    int intPart = int(v);
    if (v >= 10000.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + (intPart / 10000) % 10);
        x += sp;
    }
    if (v >= 1000.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + (intPart / 1000) % 10);
        x += sp;
    }
    if (v >= 100.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + (intPart / 100) % 10);
        x += sp;
    }
    if (v >= 10.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + (intPart / 10) % 10);
        x += sp;
    }
    result += printChar(uv, vec2(x, pos.y), scale, 48 + intPart % 10);
    x += sp;

    // Decimal point
    result += printChar(uv, vec2(x, pos.y), scale, 46);
    x += sp * 0.7;

    int dec = int(fract(v) * 100.0);
    result += printChar(uv, vec2(x, pos.y), scale, 48 + dec / 10);
    x += sp;
    result += printChar(uv, vec2(x, pos.y), scale, 48 + dec % 10);
    x += sp;

    endX = x;
    return clamp(result, 0.0, 1.0);
}

// Print a string of up to 10 characters
float printStr(vec2 uv, vec2 pos, float scale, int c[10]) {
    float result = 0.0;
    float sp = scale * 1.15;
    for (int i = 0; i < 10; i++) {
        if (c[i] == 0) break;
        result += printChar(uv, vec2(pos.x + float(i) * sp, pos.y), scale, c[i]);
    }
    return clamp(result, 0.0, 1.0);
}

// Draw a smooth horizontal bar gauge
float drawBar(vec2 uv, vec2 pos, vec2 size, float value) {
    vec2 p = (uv - pos) / size;
    if (p.x < -0.01 || p.y < -0.01 || p.x > 1.01 || p.y > 1.01) return 0.0;

    // Rounded border
    float border = 0.0;
    float bw = 0.015 / size.x;
    float bh = 0.015 / size.y;
    border += smoothstep(bw, 0.0, p.x) + smoothstep(bw, 0.0, 1.0 - p.x);
    border += smoothstep(bh, 0.0, p.y) + smoothstep(bh, 0.0, 1.0 - p.y);
    border = clamp(border, 0.0, 0.3);

    // Fill
    float fill = smoothstep(value + 0.005, value - 0.005, p.x);
    fill *= smoothstep(0.0, 0.1, p.y) * smoothstep(1.0, 0.9, p.y);
    fill *= smoothstep(0.0, 0.02, p.x);

    return border + fill;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;

    // ---- Background ----
    float vignette = 1.0 - 0.35 * length(uv - 0.5);
    vec3 col = vec3(0.04, 0.04, 0.07) * vignette;

    // Subtle grid
    float gx = smoothstep(0.97, 1.0, abs(sin(uv.x * 60.0)));
    float gy = smoothstep(0.97, 1.0, abs(sin(uv.y * 60.0)));
    col += vec3(0.02, 0.03, 0.05) * (gx + gy);

    // ---- Layout constants ----
    float scale = 0.014;
    float lineH = 0.058;
    float labelX = 0.04;
    float valueX = 0.28;
    float barX = 0.56;
    float barW = 0.38;
    float barH = 0.028;
    float startY = 0.87;

    vec3 labelCol  = vec3(0.3, 0.65, 1.0);
    vec3 valueCol  = vec3(0.85, 0.9, 0.95);
    vec3 headerCol = vec3(0.0, 1.0, 0.7);
    vec3 dimCol    = vec3(0.35, 0.4, 0.45);

    float dummy;

    // ---- Header: KROMA DEBUG ----
    float hScale = scale * 1.6;
    int hdr1[10] = int[10](75,82,79,77,65, 0,0,0,0,0);  // KROMA
    int hdr2[10] = int[10](68,69,66,85,71, 0,0,0,0,0);  // DEBUG
    float h1 = printStr(uv, vec2(0.30, 0.935), hScale, hdr1);
    float h2 = printStr(uv, vec2(0.30 + 5.5 * hScale * 1.15, 0.935), hScale, hdr2);
    col += headerCol * (h1 + h2);

    // Glow around header
    float hGlow = (h1 + h2) * 0.15;
    col += headerCol * hGlow;

    // Separator line below header
    float sepY = 0.925;
    col += headerCol * 0.4 * smoothstep(0.001, 0.0, abs(uv.y - sepY)) * smoothstep(0.1, 0.3, uv.x) * smoothstep(0.95, 0.75, uv.x);

    float y;

    // ---- Row 0: TIME ----
    y = startY;
    int lbl_time[10] = int[10](84,73,77,69, 0,0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_time);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, iTime, dummy);

    // ---- Row 1: DELTA (ms) ----
    y -= lineH;
    int lbl_delta[10] = int[10](68,69,76,84,65, 0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_delta);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, iTimeDelta * 1000.0, dummy);
    int lbl_ms[10] = int[10](77,83, 0,0,0,0,0,0,0,0);
    col += dimCol * printStr(uv, vec2(dummy + scale * 0.5, y), scale * 0.85, lbl_ms);

    // ---- Row 2: FRAME ----
    y -= lineH;
    int lbl_frame[10] = int[10](70,82,65,77,69, 0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_frame);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, float(iFrame), dummy);

    // ---- Row 3: RES (resolution) ----
    y -= lineH;
    int lbl_res[10] = int[10](82,69,83,79,76,85,84,73,79,78); // RESOLUTION
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_res);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, iResolution.x, dummy);
    int lbl_x[10] = int[10](88, 0,0,0,0,0,0,0,0,0);
    col += dimCol * printStr(uv, vec2(dummy, y), scale * 0.85, lbl_x);
    col += valueCol * printFloat(uv, vec2(dummy + scale * 1.3, y), scale, iResolution.y, dummy);

    // ---- Row 4: MOUSE ----
    y -= lineH;
    int lbl_mouse[10] = int[10](77,79,85,83,69, 0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_mouse);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, iMouse.x, dummy);
    col += dimCol * printStr(uv, vec2(dummy, y), scale * 0.85, lbl_x);
    col += valueCol * printFloat(uv, vec2(dummy + scale * 1.3, y), scale, iMouse.y, dummy);

    // ---- Separator ----
    y -= lineH * 0.4;
    col += labelCol * 0.2 * smoothstep(0.001, 0.0, abs(uv.y - y)) * smoothstep(0.02, 0.1, uv.x) * smoothstep(0.98, 0.9, uv.x);

    // ---- Row 5: CPU ----
    y -= lineH * 0.6;
    int lbl_cpu[10] = int[10](67,80,85, 0,0,0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_cpu);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, u_cpu * 100.0, dummy);
    int lbl_pct[10] = int[10](37, 0,0,0,0,0,0,0,0,0);
    col += dimCol * printStr(uv, vec2(dummy, y), scale * 0.85, lbl_pct);
    float cpuBar = drawBar(uv, vec2(barX, y - 0.003), vec2(barW, barH), u_cpu);
    col += mix(vec3(0.1, 0.85, 0.3), vec3(1.0, 0.2, 0.1), u_cpu) * cpuBar;

    // ---- Row 6: RAM ----
    y -= lineH;
    int lbl_ram[10] = int[10](82,65,77, 0,0,0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_ram);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, u_ram * 100.0, dummy);
    col += dimCol * printStr(uv, vec2(dummy, y), scale * 0.85, lbl_pct);
    float ramBar = drawBar(uv, vec2(barX, y - 0.003), vec2(barW, barH), u_ram);
    col += mix(vec3(0.15, 0.5, 0.9), vec3(0.9, 0.2, 0.15), u_ram) * ramBar;

    // ---- Row 7: BATTERY ----
    y -= lineH;
    int lbl_batt[10] = int[10](66,65,84,84,69,82,89, 0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_batt);
    if (u_battery >= 0.0) {
        col += valueCol * printFloat(uv, vec2(valueX, y), scale, u_battery * 100.0, dummy);
        col += dimCol * printStr(uv, vec2(dummy, y), scale * 0.85, lbl_pct);
        float battBar = drawBar(uv, vec2(barX, y - 0.003), vec2(barW, barH), u_battery);
        vec3 battCol = u_battery < 0.2 ? vec3(1.0, 0.1, 0.1)
                     : u_battery < 0.5 ? vec3(1.0, 0.7, 0.0)
                     :                    vec3(0.1, 0.9, 0.25);
        col += battCol * battBar;
    } else {
        int lbl_na[10] = int[10](78,47,65, 0,0,0,0,0,0,0);
        col += dimCol * printStr(uv, vec2(valueX, y), scale, lbl_na);
    }

    // ---- Row 8: AUDIO ----
    y -= lineH;
    int lbl_audio[10] = int[10](65,85,68,73,79, 0,0,0,0,0);
    col += labelCol * printStr(uv, vec2(labelX, y), scale, lbl_audio);
    col += valueCol * printFloat(uv, vec2(valueX, y), scale, u_audio_level, dummy);
    float audioBar = drawBar(uv, vec2(barX, y - 0.003), vec2(barW, barH), u_audio_level);
    col += vec3(0.2, 1.0, 0.55) * audioBar;

    // ---- Footer separator ----
    y -= lineH * 0.5;
    col += labelCol * 0.15 * smoothstep(0.001, 0.0, abs(uv.y - y)) * smoothstep(0.02, 0.1, uv.x) * smoothstep(0.98, 0.9, uv.x);

    // ---- Panel background (darken behind text) ----
    float panelLeft = 0.02;
    float panelRight = 0.98;
    float panelTop = 0.96;
    float panelBot = y - 0.01;
    float inPanel = smoothstep(panelLeft - 0.01, panelLeft + 0.01, uv.x)
                  * smoothstep(panelRight + 0.01, panelRight - 0.01, uv.x)
                  * smoothstep(panelBot - 0.01, panelBot + 0.01, uv.y)
                  * smoothstep(panelTop + 0.01, panelTop - 0.01, uv.y);
    // Apply panel as a darkening layer behind (mix with background)
    vec3 panelBg = vec3(0.02, 0.025, 0.04);
    col = mix(col, col + panelBg * 0.3, inPanel * 0.5);

    // ---- Subtle CRT scanline effect ----
    col *= 0.96 + 0.04 * sin(fragCoord.y * 3.14159);

    // ---- Subtle chromatic glow on edges ----
    col += vec3(0.01, 0.02, 0.04) * smoothstep(0.6, 0.0, length(uv - 0.5));

    fragColor = vec4(col, 1.0);
}
