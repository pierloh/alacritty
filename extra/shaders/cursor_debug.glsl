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

    // Estimate cell size from cursor dimensions.
    float cellH = iCurrentCursor.w;
    float cellW = max(iCurrentCursor.z, cellH * 0.5);

    // Multi-cursor mode (kitty protocol).
    if (iCursorCount > 0) {
        int count = min(iCursorCount, MAX_CURSORS);

        // Previous positions: red.
        for (int i = 0; i < count; i++) {
            vec2 prev = iPreviousCursors[i].xy;
            float pW = iPreviousCursors[i].z;
            float pH = iPreviousCursors[i].w;
            if (pW > 0.0 &&
                fragCoord.x >= prev.x && fragCoord.x <= prev.x + pW &&
                fragCoord.y <= prev.y && fragCoord.y >= prev.y - pH) {
                fragColor = vec4(mix(color.rgb, vec3(1.0, 0.0, 0.0), 0.3), color.a);
                return;
            }
        }

        // Current positions: green.
        for (int i = 0; i < count; i++) {
            vec2 pos = iCursors[i].xy;
            float cW = iCursors[i].z;
            float cH = iCursors[i].w;
            if (fragCoord.x >= pos.x && fragCoord.x <= pos.x + cW &&
                fragCoord.y <= pos.y && fragCoord.y >= pos.y - cH) {
                fragColor = vec4(mix(color.rgb, vec3(0.0, 1.0, 0.0), 0.4), color.a);
                return;
            }
        }

        fragColor = color;
        return;
    }

    // Single-cursor fallback (insert mode).
    if (iCursorVisible > 0.0) {
        vec2 prev = iPreviousCursor.xy;
        if (prev.x >= 0.0 &&
            fragCoord.x >= prev.x && fragCoord.x <= prev.x + cellW &&
            fragCoord.y <= prev.y && fragCoord.y >= prev.y - cellH) {
            fragColor = vec4(mix(color.rgb, vec3(1.0, 0.0, 0.0), 0.3), color.a);
            return;
        }

        vec2 cur = iCurrentCursor.xy;
        if (fragCoord.x >= cur.x && fragCoord.x <= cur.x + cellW &&
            fragCoord.y <= cur.y && fragCoord.y >= cur.y - cellH) {
            fragColor = vec4(mix(color.rgb, vec3(0.0, 1.0, 0.0), 0.4), color.a);
            return;
        }
    }

    fragColor = color;
}
