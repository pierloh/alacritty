#pragma include "lib.glsl"

// CONFIGURATION
const float DURATION = 0.3;                // How long the effect animates (seconds)
const float MAX_SIZE = 0.12;               // Max expansion in normalized coords
const float RING_THICKNESS = 0.02;         // Ring width (ignored when FILLED = true)
vec4 COLOR = iCurrentCursorColor;          // Effect color (inherit cursor color)
const float BLUR = 3.0;                    // Anti-alias blur in pixels
const float ANIMATION_START_OFFSET = 0.0;  // Start slightly progressed (0.0 - 1.0)

// SHAPE
const bool RECTANGULAR = false;            // true = rectangle, false = circle
const bool FILLED = false;                 // true = solid boom, false = ring ripple

// TRIGGERS
const bool TRIGGER_ON_JUMP = false;        // Large cursor jumps (multi-line, multi-cell)
const bool TRIGGER_ON_MODE_CHANGE = true;  // Cursor mode change (e.g. vim insert <-> normal)

// HOLDOUT -- punch out cursor shape from the effect
const bool CURSOR_HOLDOUT = false;         // true = effect doesn't render on top of cursor

float effect(vec2 frag, vec2 center, vec2 halfSize, float expansion, float aa) {
    float sdf;
    if (RECTANGULAR) {
        sdf = sdfRect(frag, center, halfSize + vec2(expansion));
    } else {
        sdf = distance(frag, center) - expansion;
    }
    if (!FILLED) sdf = abs(sdf) - RING_THICKNESS * 0.5;
    return 1.0 - smoothstep(-aa, aa, sdf);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 original = texture(iChannel0, fragCoord / iResolution.xy);
    fragColor = original;

    vec2 vu = norm(fragCoord, 1.0);

    float progress = (iTime - iTimeCursorChange) / DURATION + ANIMATION_START_OFFSET;
    if (progress >= 1.0) return;

    // Mode change uses its own timestamp (DECTCEM, not affected by blink).
    float modeProgress = (iTime - iTimeModeChange) / DURATION;
    float shouldTrigger = (TRIGGER_ON_MODE_CHANGE && modeProgress < 1.0) ? 1.0 : 0.0;

    float easedProgress = easeOutCirc(progress);
    float expansion = easedProgress * MAX_SIZE;
    float fade = 1.0 - easeOutPulse(progress);
    float aa = norm(vec2(BLUR), 0.0).x;

    if (iCursorCount > 0) {
        int count = min(iCursorCount, MAX_CURSORS);
        float cellH = norm(iCursors[0].zw, 0.0).y;
        float cellW = cellH * 0.5;

        if (TRIGGER_ON_JUMP) {
            for (int i = 0; i < count; i++)
                shouldTrigger = max(shouldTrigger, detectJump(
                    norm(iCursors[i].xy, 1.0), norm(iPreviousCursors[i].xy, 1.0), cellH, cellW));
        }
        if (shouldTrigger == 0.0) return;

        float maxEffect = 0.0;
        for (int i = 0; i < count; i++) {
            vec2 cSize = norm(iCursors[i].zw, 0.0);
            vec2 center = cellCenter(norm(iCursors[i].xy, 1.0), cSize);
            maxEffect = max(maxEffect, effect(vu, center, cSize * 0.5, expansion, aa));
        }
        fragColor = mix(fragColor, COLOR, maxEffect * fade * COLOR.a);

    } else {
        vec2 curPos = norm(iCurrentCursor.xy, 1.0);
        vec2 prevPos = norm(iPreviousCursor.xy, 1.0);
        vec2 cellSize = norm(iCellSize, 0.0);

        if (TRIGGER_ON_JUMP)
            shouldTrigger = max(shouldTrigger, detectJump(curPos, prevPos, cellSize.y, cellSize.x));
        if (shouldTrigger == 0.0) return;

        vec2 center = cellCenter(curPos, cellSize);
        float e = effect(vu, center, cellSize * 0.5, expansion, aa);
        fragColor = mix(fragColor, COLOR, e * fade * COLOR.a);
    }

    if (CURSOR_HOLDOUT) fragColor = cursorHoldout(fragColor, original, fragCoord);
}
