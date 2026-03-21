#pragma include "lib.glsl"

// CONFIGURATION
vec4 TRAIL_COLOR = iCurrentCursorColor;    // Trail color (inherit cursor color)
const float DURATION = 0.2;                // Animation duration (seconds)
const float TRAIL_LENGTH = 0.5;            // Initial trail length as fraction of move (SWEEP mode)
const float BLUR = 2.0;                    // Anti-alias blur in pixels
const float MIN_DISTANCE = 1.5;            // Min move distance to show trail (cursor-width units)
const bool SKIP_SINGLE_CELL_MOVE = true;     // Skip effect for single-cell moves (typing, arrow keys)

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

// EFFECT SHAPE -- use cell dimensions instead of cursor shape for the trail
const bool USE_CELL_SHAPE = false;  // true = cell-sized effect, false = follows cursor shape

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

    if (iCursorCount == 0 && iCursorVisible == 0.0) return;

    vec2 vu = norm(fragCoord, 1.0);

    int sweepCount = (iCursorCount > 0) ? min(iCursorCount, MAX_CURSORS) : 1;
    for (int ci = 0; ci < sweepCount; ci++) {

    vec4 cur, prev;
    vec2 cellSize;
    if (iCursorCount > 0) {
        cur = vec4(norm(iCursors[ci].xy, 1.0), norm(iCursors[ci].zw, 0.0));
        prev = vec4(norm(iPreviousCursors[ci].xy, 1.0), norm(iPreviousCursors[ci].zw, 0.0));
        cellSize = cur.zw;  // multi-cursor: zw is already cell-sized
    } else {
        cur = vec4(norm(iCurrentCursor.xy, 1.0), norm(iCurrentCursor.zw, 0.0));
        prev = vec4(norm(iPreviousCursor.xy, 1.0), norm(iPreviousCursor.zw, 0.0));
        cellSize = norm(iCellSize, 0.0);
    }

    vec2 cC = cellCenter(cur.xy, cellSize);
    vec2 cP = cellCenter(prev.xy, cellSize);
    float lineLen = distance(cC, cP);

    vec4 outC = fragColor;

    float progress = clamp((iTime - iTimeCursorChange) / DURATION, 0.0, 1.0);
    if (lineLen > cellSize.y * MIN_DISTANCE) {

        // Skip single horizontal cell moves (typing, arrow keys).
        if (SKIP_SINGLE_CELL_MOVE) {
            vec2 curPos, prevPos;
            if (iCursorCount > 0) {
                curPos = norm(iCursors[ci].xy, 1.0);
                prevPos = norm(iPreviousCursors[ci].xy, 1.0);
            } else {
                curPos = norm(iCurrentCursor.xy, 1.0);
                prevPos = norm(iPreviousCursor.xy, 1.0);
            }
            if (detectJumpCell(curPos, prevPos, cellSize.y, cellSize.x) == 0.0) continue;
        }

        // Detect straight (axis-aligned) moves.
        vec2 d = abs(cC - cP);
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
        vec2 headPos = mix(prev.xy, cur.xy, headT);
        vec2 tailPos = mix(prev.xy, cur.xy, tailT);

        float trFlag = topRightLeads(cur.xy, prev.xy);
        float blFlag = 1.0 - trFlag;

        vec2 effectSize = USE_CELL_SHAPE ? cellSize : cur.zw;
        vec2 prevEffectSize = USE_CELL_SHAPE ? cellSize : prev.zw;
        vec2 v0 = vec2(headPos.x + effectSize.x * trFlag, headPos.y - effectSize.y);
        vec2 v1 = vec2(headPos.x + effectSize.x * blFlag, headPos.y);
        vec2 v2 = vec2(tailPos.x + effectSize.x * blFlag, tailPos.y);
        vec2 v3 = vec2(tailPos.x + effectSize.x * trFlag, tailPos.y - prevEffectSize.y);

        // CW winding to match expected SDF sign (negative inside).
        float sdfDiag = sdfQuad(vu, v0, v3, v2, v1);

        // --- Rectangle SDF (straight moves) ---
        vec2 headCenter = mix(cP, cC, headT);
        vec2 tailCenter = mix(cP, cC, tailT);
        vec2 boxMin = min(headCenter, tailCenter);
        vec2 boxMax = max(headCenter, tailCenter);
        vec2 boxSize = (boxMax - boxMin) + effectSize;
        vec2 boxCenter = (boxMin + boxMax) * 0.5;

        float sdfStraight = sdfRect(vu, boxCenter, boxSize * 0.5);

        // --- Draw ---
        float sdfTrail = mix(sdfDiag, sdfStraight, isStraight);
        float trailAlpha = antialias(sdfTrail);

        // Spatial gradient: project fragment onto tail->head axis.
        if (TRAIL_GRADIENT && tailT < headT) {
            vec2 tc = mix(cP, cC, tailT);
            vec2 hc = mix(cP, cC, headT);
            vec2 axis = hc - tc;
            float axisLen = dot(axis, axis);
            float t = (axisLen > 0.0001) ? clamp(dot(vu - tc, axis) / axisLen, 0.0, 1.0) : 1.0;
            trailAlpha *= pow(t, GRADIENT_POWER);
        }

        outC = mix(outC, TRAIL_COLOR, trailAlpha);

    }

    fragColor = outC;
    if (CURSOR_HOLDOUT) fragColor = cursorHoldout(fragColor, original, fragCoord);

    } // end sweepCount loop
}
