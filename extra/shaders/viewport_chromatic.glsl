// Chromatic aberration - splits RGB channels radially from screen center.

// CONFIGURATION
const float INTENSITY = 1.5;               // Channel offset in pixels

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    // Offset direction: radial from screen center.
    vec2 shift = (uv - 0.5) * INTENSITY / iResolution.xy;

    float r = texture(iChannel0, uv + shift).r;
    float g = texture(iChannel0, uv).g;
    float b = texture(iChannel0, uv - shift).b;
    float a = texture(iChannel0, uv).a;

    fragColor = vec4(r, g, b, a);
}
