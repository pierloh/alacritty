#pragma include "lib.glsl"

// CONFIGURATION
vec4 TRAIL_COLOR = iCurrentCursorColor;    // Trail color (inherit cursor color)
const float DURATION = 0.2;                // Animation duration (seconds)
const float TRAIL_LENGTH = 0.5;            // Initial trail length as fraction of move (SWEEP mode)
const float BLUR = 2.0;                    // Anti-alias blur in pixels
const float MIN_DISTANCE = 1.5;            // Min move distance to show trail (cursor-width units)
// MODE
// false = SWEEP: trail appears at TRAIL_LENGTH and shrinks toward cursor
// true  = TAIL:  head snaps to cursor, tail catches up with delay
const bool TAIL_MODE = false;
const float MAX_TAIL_LENGTH = 0.2;         // Max tail length in normalized coords (TAIL mode)

// GRADIENT -- fade trail from head to tail
const bool TRAIL_GRADIENT = true;          // true = fade alpha along trail length
const float GRADIENT_POWER = 1.0;          // >1 = faster fade toward tail, <1 = slower

// HOLDOUT -- punch out cursor shape from the effect
const bool CURSOR_HOLDOUT = true;          // true = trail doesn't render on top of cursor

float antialias(float d) {
    return 1.0 - smoothstep(0.0, norm(vec2(BLUR), 0.0).x, d);
}

// Determine which diagonal corner leads the movement.
float topRightLeads(vec2 a, vec2 b) {
    float c1 = step(b.x, a.x) * step(a.y, b.y);
    float c2 = step(a.x, b.x) * step(b.y, a.y);
    return 1.0 - max(c1, c2);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 original = texture(iChannel0, fragCoord / iResolution.xy);
    fragColor = original;

    if (iCurrentCursorCount == 0) return;

    vec2 vu = norm(fragCoord, 1.0);

    int cursorCount = min(iCurrentCursorCount, MAX_CURSORS);
    for (int ci = 0; ci < cursorCount; ci++) {

    vec4 currentCursor = vec4(norm(iCurrentCursors[ci].xy, 1.0), norm(iCurrentCursors[ci].zw, 0.0));
    vec4 previousCursor = vec4(norm(iPreviousCursors[ci].xy, 1.0), norm(iPreviousCursors[ci].zw, 0.0));

    vec2 currentCenter = cursorCenter(currentCursor.xy, currentCursor.zw);
    vec2 previousCenter = cursorCenter(previousCursor.xy, previousCursor.zw);
    float lineLen = distance(currentCenter, previousCenter);

    vec4 outC = fragColor;

    float progress = clamp((iTime - iTimeCursorChange) / DURATION, 0.0, 1.0);
    if (lineLen > currentCursor.w * MIN_DISTANCE) {

        // Detect straight (axis-aligned) moves.
        vec2 d = abs(currentCenter - previousCenter);
        float isStraight = max(step(d.y, 0.001), step(d.x, 0.001));

        // Compute head/tail progress along the path.
        float headT, tailT;
        if (TAIL_MODE) {
            float delayFactor = MAX_TAIL_LENGTH / lineLen;
            float isLong = step(MAX_TAIL_LENGTH, lineLen);
            float headShort = easeOutCirc(progress);
            float tailShort = easeOutCirc(smoothstep(delayFactor, 1.0, progress));
            headT = mix(1.0, headShort, isLong);
            tailT = mix(easeOutCirc(progress), tailShort, isLong);
        } else {
            float shrink = easeOutCubic(progress);
            headT = 1.0;
            tailT = mix(1.0 - TRAIL_LENGTH, 1.0, shrink);
        }

        // --- Parallelogram SDF (diagonal moves) ---
        vec2 headPos = mix(previousCursor.xy, currentCursor.xy, headT);
        vec2 tailPos = mix(previousCursor.xy, currentCursor.xy, tailT);

        float trFlag = topRightLeads(currentCursor.xy, previousCursor.xy);
        float blFlag = 1.0 - trFlag;

        vec2 currentEffectSize = currentCursor.zw;
        vec2 previousEffectSize = previousCursor.zw;
        vec2 v0 = vec2(headPos.x + currentEffectSize.x * trFlag, headPos.y - currentEffectSize.y);
        vec2 v1 = vec2(headPos.x + currentEffectSize.x * blFlag, headPos.y);
        vec2 v2 = vec2(tailPos.x + currentEffectSize.x * blFlag, tailPos.y);
        vec2 v3 = vec2(tailPos.x + currentEffectSize.x * trFlag, tailPos.y - previousEffectSize.y);

        // CW winding to match expected SDF sign (negative inside).
        float sdfDiag = sdfQuad(vu, v0, v3, v2, v1);

        // --- Rectangle SDF (straight moves) ---
        vec2 headCenter = mix(previousCenter, currentCenter, headT);
        vec2 tailCenter = mix(previousCenter, currentCenter, tailT);
        vec2 boxMin = min(headCenter, tailCenter);
        vec2 boxMax = max(headCenter, tailCenter);
        vec2 boxSize = (boxMax - boxMin) + currentEffectSize;
        vec2 boxCenter = (boxMin + boxMax) * 0.5;

        float sdfStraight = sdfRect(vu, boxCenter, boxSize * 0.5);

        // --- Draw ---
        float sdfTrail = mix(sdfDiag, sdfStraight, isStraight);
        float trailAlpha = antialias(sdfTrail);

        // Spatial gradient: project fragment onto tail->head axis.
        if (TRAIL_GRADIENT && tailT < headT) {
            vec2 tc = mix(previousCenter, currentCenter, tailT);
            vec2 hc = mix(previousCenter, currentCenter, headT);
            vec2 axis = hc - tc;
            float axisLen = dot(axis, axis);
            float t = (axisLen > 0.0001) ? clamp(dot(vu - tc, axis) / axisLen, 0.0, 1.0) : 1.0;
            trailAlpha *= pow(t, GRADIENT_POWER);
        }

        outC = mix(outC, TRAIL_COLOR, trailAlpha);

    }

    fragColor = outC;
    if (CURSOR_HOLDOUT) fragColor = cursorHoldout(fragColor, original, fragCoord);

    } // end cursor loop
}
