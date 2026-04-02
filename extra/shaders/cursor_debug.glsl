// Debug shader: draws cell-sized rects at cursor positions.
// Green = current cursors, red = previous positions.
// Yellow bar = time since last cursor move.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution;
    vec4 color = texture(iChannel0, uv);

    // Time since move bar: full width = 10 seconds, with 1-second tick marks.
    float timeSinceMove = iTime - iTimeCursorChange;
    float barWidth = (timeSinceMove / 10.0) * iResolution.x;
    float tickSpacing = iResolution.x / 10.0;
    if (fragCoord.y < 5.0 && fragCoord.x < barWidth) {
        float tick = mod(fragCoord.x, tickSpacing);
        if (tick > tickSpacing - 2.0) {
            fragColor = color;
        } else {
            float pulse = 0.8 + 0.2 * sin(timeSinceMove * 6.2831853);
            fragColor = vec4(pulse, pulse, 0.0, 1.0);
        }
        return;
    }

    int count = min(iCurrentCursorCount, MAX_CURSORS);

    // Previous positions: red.
    int previousCount = min(iPreviousCursorCount, MAX_CURSORS);
    for (int i = 0; i < previousCount; i++) {
        vec2 previousPos = iPreviousCursors[i].xy;
        float previousW = iPreviousCursors[i].z;
        float previousH = iPreviousCursors[i].w;
        if (previousW > 0.0 &&
            fragCoord.x >= previousPos.x && fragCoord.x <= previousPos.x + previousW &&
            fragCoord.y <= previousPos.y && fragCoord.y >= previousPos.y - previousH) {
            fragColor = vec4(mix(color.rgb, vec3(1.0, 0.0, 0.0), 0.3), color.a);
            return;
        }
    }

    // Current positions: green.
    for (int i = 0; i < count; i++) {
        vec2 currentPos = iCurrentCursors[i].xy;
        float currentW = iCurrentCursors[i].z;
        float currentH = iCurrentCursors[i].w;
        if (fragCoord.x >= currentPos.x && fragCoord.x <= currentPos.x + currentW &&
            fragCoord.y <= currentPos.y && fragCoord.y >= currentPos.y - currentH) {
            fragColor = vec4(mix(color.rgb, vec3(0.0, 1.0, 0.0), 0.4), color.a);
            return;
        }
    }

    fragColor = color;
}
