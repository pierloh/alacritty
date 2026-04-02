#pragma include "lib.glsl"

// CONFIGURATION
const float GLOW_SPREAD = 40.0;            // Glow radius in pixels
const float GLOW_INTENSITY = 0.15;         // Glow brightness
const bool WEIGHT_BY_TYPE = false;         // true = primary cursors brighter than secondary
const bool CURSOR_HOLDOUT = true;          // true = glow doesn't render on top of cursor

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 original = texture(iChannel0, fragCoord / iResolution);
    vec4 color = original;
    float glow = 0.0;

    int count = min(iCurrentCursorCount, MAX_CURSORS);
    for (int i = 0; i < count; i++) {
        vec2 center = iCurrentCursors[i].xy + iCurrentCursors[i].zw * vec2(0.5, -0.5);
        float dist = sdfRect(fragCoord, center, iCurrentCursors[i].zw * 0.5);
        float weight = (WEIGHT_BY_TYPE && iCurrentCursorTypes[i] != 0) ? 0.5 : 1.0;
        glow += weight * smoothstep(GLOW_SPREAD, 0.0, dist) * GLOW_INTENSITY;
    }

    fragColor = vec4(mix(color.rgb, iCurrentCursorColor.rgb, min(glow, 1.0)), color.a);

    if (CURSOR_HOLDOUT) fragColor = cursorHoldout(fragColor, original, fragCoord);
}
