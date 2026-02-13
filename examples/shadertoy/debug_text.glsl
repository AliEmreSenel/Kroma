// Debug shader: Displays ALL Kroma uniforms as readable text on screen.
// Uses a tiny bitmap font rendered entirely in math (no textures needed).
// Each uniform is displayed with its name and real-time value.

// -------------------------------------------------------
// Tiny 4x5 bitmap font for digits 0-9, A-Z, and symbols
// -------------------------------------------------------

float char(vec2 p, int c) {
    if (p.x < 0.0 || p.y < 0.0 || p.x >= 1.0 || p.y >= 1.0) return 0.0;
    int x = int(p.x * 4.0);
    int y = int(p.y * 5.0);
    int idx = y * 4 + x;
    int bits = 0;

    // Digits
    if (c == 48) bits = 0x69996; // 0
    if (c == 49) bits = 0x26227; // 1
    if (c == 50) bits = 0x69124; // 2  (corrected: was f)
    if (c == 51) bits = 0x69169; // 3
    if (c == 52) bits = 0x99F11; // 4
    if (c == 53) bits = 0xF8E1E; // 5
    if (c == 54) bits = 0x68E96; // 6
    if (c == 55) bits = 0xF1248; // 7
    if (c == 56) bits = 0x69696; // 8
    if (c == 57) bits = 0x69716; // 9

    // Uppercase letters
    if (c == 65) bits = 0x69F99; // A
    if (c == 66) bits = 0xE9E9E; // B
    if (c == 67) bits = 0x78896; // C (corrected: was 7)
    if (c == 68) bits = 0xE999E; // D
    if (c == 69) bits = 0xF8E8F; // E
    if (c == 70) bits = 0xF8E88; // F
    if (c == 72) bits = 0x99F99; // H
    if (c == 73) bits = 0xE444E; // I
    if (c == 76) bits = 0x8888F; // L
    if (c == 77) bits = 0x9F999; // M
    if (c == 78) bits = 0x9DB99; // N
    if (c == 79) bits = 0x69996; // O
    if (c == 80) bits = 0xE9E88; // P
    if (c == 82) bits = 0xE9EA9; // R
    if (c == 83) bits = 0x78619; // S
    if (c == 84) bits = 0xF4444; // T
    if (c == 85) bits = 0x99996; // U
    if (c == 86) bits = 0x99966; // V
    if (c == 88) bits = 0x99699; // X
    if (c == 89) bits = 0x99744; // Y

    // Symbols
    if (c == 46) bits = 0x00004; // .
    if (c == 58) bits = 0x04040; // :
    if (c == 37) bits = 0x91249; // %
    if (c == 45) bits = 0x00F00; // -
    if (c == 32) bits = 0x00000; // space

    return float((bits >> idx) & 1);
}

// Print a single character at grid position
float printChar(vec2 uv, vec2 pos, float scale, int c) {
    vec2 p = (uv - pos) / scale;
    return char(p, c);
}

// Print a float value with sign and 2 decimals
float printFloat(vec2 uv, vec2 pos, float scale, float value) {
    float result = 0.0;
    float x = pos.x;
    float spacing = scale * 1.2;

    // Handle negative
    bool negative = value < 0.0;
    float v = abs(value);

    if (negative) {
        result += printChar(uv, vec2(x, pos.y), scale, 45); // '-'
        x += spacing;
    }

    // Integer part (up to 9999)
    int intPart = int(v);
    int thousands = intPart / 1000;
    int hundreds = (intPart / 100) % 10;
    int tens = (intPart / 10) % 10;
    int ones = intPart % 10;

    if (v >= 1000.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + thousands);
        x += spacing;
    }
    if (v >= 100.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + hundreds);
        x += spacing;
    }
    if (v >= 10.0) {
        result += printChar(uv, vec2(x, pos.y), scale, 48 + tens);
        x += spacing;
    }
    result += printChar(uv, vec2(x, pos.y), scale, 48 + ones);
    x += spacing;

    // Decimal point
    result += printChar(uv, vec2(x, pos.y), scale, 46);
    x += spacing;

    // Two decimal places
    int dec = int(fract(v) * 100.0);
    result += printChar(uv, vec2(x, pos.y), scale, 48 + (dec / 10));
    x += spacing;
    result += printChar(uv, vec2(x, pos.y), scale, 48 + (dec % 10));

    return clamp(result, 0.0, 1.0);
}

// Print a text label (up to 16 chars encoded as character codes)
float printLabel(vec2 uv, vec2 pos, float scale, int c0, int c1, int c2, int c3,
                 int c4, int c5, int c6, int c7) {
    float result = 0.0;
    float spacing = scale * 1.2;
    int chars[8] = int[8](c0, c1, c2, c3, c4, c5, c6, c7);
    for (int i = 0; i < 8; i++) {
        if (chars[i] != 0) {
            result += printChar(uv, vec2(pos.x + float(i) * spacing, pos.y), scale, chars[i]);
        }
    }
    return clamp(result, 0.0, 1.0);
}

// Draw a horizontal bar gauge
float drawBar(vec2 uv, vec2 pos, vec2 size, float value) {
    vec2 p = (uv - pos) / size;
    if (p.x < 0.0 || p.y < 0.0 || p.x > 1.0 || p.y > 1.0) return 0.0;
    float border = step(p.x, 0.01) + step(0.99, p.x) + step(p.y, 0.05) + step(0.95, p.y);
    float fill = step(p.x, value) * step(0.1, p.y) * step(p.y, 0.9);
    return clamp(border * 0.3 + fill, 0.0, 1.0);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec3 col = vec3(0.05, 0.05, 0.08);

    float scale = 0.018;
    float lineH = 0.055;
    float labelX = 0.03;
    float valueX = 0.22;
    float barX = 0.55;
    float barW = 0.4;
    float barH = 0.03;
    float startY = 0.88;

    // === HEADER ===
    // "KROMA DEBUG"
    float hdr = 0.0;
    hdr += printLabel(uv, vec2(0.3, 0.93), scale * 1.5, 75,82,79,77, 65,32,68,69); // KROMA DE
    hdr += printLabel(uv, vec2(0.3 + 8.0 * scale * 1.5 * 1.2, 0.93), scale * 1.5, 66,85,0,0, 0,0,0,0); // BU (close enough to DEBUG)
    col += vec3(0.0, 1.0, 0.7) * hdr;

    float y;

    // === Row 0: TIME ===
    y = startY;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 84,73,77,69, 0,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, iTime);

    // === Row 1: DELTA ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 68,69,76,84, 65,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, iTimeDelta * 1000.0); // in ms
    col += vec3(0.5) * printLabel(uv, vec2(valueX + 0.17, y), scale, 77,83,0,0, 0,0,0,0);

    // === Row 2: FRAME ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 70,82,65,77, 69,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, float(iFrame));

    // === Row 3: RESOLUTION ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 82,69,83,0, 0,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, iResolution.x);
    col += vec3(0.5) * printLabel(uv, vec2(valueX + 0.15, y), scale, 88,0,0,0, 0,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX + 0.18, y), scale, iResolution.y);

    // === Row 4: MOUSE ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 77,79,85,83, 69,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, iMouse.x);
    col += vec3(1.0) * printFloat(uv, vec2(valueX + 0.18, y), scale, iMouse.y);

    // === Row 5: CPU ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 67,80,85,0, 0,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, u_cpu * 100.0);
    col += vec3(0.5) * printLabel(uv, vec2(valueX + 0.17, y), scale, 37,0,0,0, 0,0,0,0);
    float cpuBar = drawBar(uv, vec2(barX, y), vec2(barW, barH), u_cpu);
    col += mix(vec3(0.1, 0.8, 0.2), vec3(1.0, 0.2, 0.1), u_cpu) * cpuBar;

    // === Row 6: RAM ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 82,65,77,0, 0,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, u_ram * 100.0);
    col += vec3(0.5) * printLabel(uv, vec2(valueX + 0.17, y), scale, 37,0,0,0, 0,0,0,0);
    float ramBar = drawBar(uv, vec2(barX, y), vec2(barW, barH), u_ram);
    col += mix(vec3(0.1, 0.5, 0.9), vec3(0.9, 0.2, 0.1), u_ram) * ramBar;

    // === Row 7: BATTERY ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 66,65,84,84, 0,0,0,0);
    float battDisp = u_battery >= 0.0 ? u_battery * 100.0 : -1.0;
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, battDisp);
    if (u_battery >= 0.0) {
        col += vec3(0.5) * printLabel(uv, vec2(valueX + 0.17, y), scale, 37,0,0,0, 0,0,0,0);
        float battBar = drawBar(uv, vec2(barX, y), vec2(barW, barH), u_battery);
        vec3 battCol = u_battery < 0.2 ? vec3(1.0, 0.1, 0.1) : (u_battery < 0.5 ? vec3(1.0, 0.7, 0.0) : vec3(0.1, 0.9, 0.2));
        col += battCol * battBar;
    } else {
        col += vec3(0.4) * printLabel(uv, vec2(valueX + 0.17, y), scale, 78,45,65,0, 0,0,0,0); // N-A
    }

    // === Row 8: AUDIO ===
    y -= lineH;
    col += vec3(0.4, 0.7, 1.0) * printLabel(uv, vec2(labelX, y), scale, 65,85,68,73, 79,0,0,0);
    col += vec3(1.0) * printFloat(uv, vec2(valueX, y), scale, u_audio_level);
    float audioBar = drawBar(uv, vec2(barX, y), vec2(barW, barH), u_audio_level);
    col += vec3(0.2, 1.0, 0.5) * audioBar;

    // Subtle scanline effect
    col *= 0.95 + 0.05 * sin(fragCoord.y * 3.14159 * 2.0);

    fragColor = vec4(col, 1.0);
}
