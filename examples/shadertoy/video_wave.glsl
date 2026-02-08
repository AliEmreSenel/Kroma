// Shadertoy example with texture channels — tests channel detection.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Sample video texture on channel 0
    vec4 videoColor = texture(iChannel0, uv);

    // Simple wave distortion based on time
    float wave = sin(uv.y * 20.0 + iTime * 3.0) * 0.01;
    vec4 distorted = texture(iChannel0, uv + vec2(wave, 0.0));

    // Blend with a time-based color shift
    vec3 tint = vec3(
        0.5 + 0.5 * sin(iTime),
        0.5 + 0.5 * sin(iTime + 2.094),
        0.5 + 0.5 * sin(iTime + 4.189)
    );

    fragColor = vec4(mix(distorted.rgb, tint, 0.15), 1.0);
}
