#pragma include "lib.glsl"

// Neovide-style cursor warp/smear trail.
// Original by sahaj-b (ghostty-cursor-shaders, MIT license).
// Only change: "normalize" renamed to "norm" to avoid GLSL builtin conflict.

// --- CONFIGURATION ---
vec4 TRAIL_COLOR = iCurrentCursorColor;
const float DURATION = 0.2;
const float TRAIL_SIZE = 0.8;
const float THRESHOLD_MIN_DISTANCE = 1.5;
const float BLUR = 1.0;
const float TRAIL_THICKNESS = 1.0;
const float TRAIL_THICKNESS_X = 0.9;

const float FADE_ENABLED = 0.0;
const float FADE_EXPONENT = 5.0;

// HOLDOUT -- punch out cursor shape from the effect
const bool CURSOR_HOLDOUT = true;          // true = effect doesn't render on top of cursor

// EFFECT SHAPE -- cell vs cursor shape for visual rendering
const bool USE_CELL_SHAPE = false;  // true = cell-sized effect, false = follows cursor shape

const bool SKIP_SINGLE_CELL_MOVE = true;     // Skip effect for single-cell moves (typing, arrow keys)

const float PI = 3.14159265359;

// EaseOutCirc
float ease(float x) {
    return sqrt(1.0 - pow(x - 1.0, 2.0));
}

float antialias(float distance, float blurAmount) {
    return 1.0 - smoothstep(0.0, norm(vec2(blurAmount), 0.0).x, distance);
}

float getDurationFromDot(float dot_val, float DURATION_LEAD, float DURATION_SIDE, float DURATION_TRAIL) {
    float isLead = step(0.5, dot_val);
    float isSide = step(-0.5, dot_val) * (1.0 - isLead);
    float duration = mix(DURATION_TRAIL, DURATION_SIDE, isSide);
    duration = mix(duration, DURATION_LEAD, isLead);
    return duration;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 original = texture(iChannel0, fragCoord.xy / iResolution.xy);
    fragColor = original;

    // Cursor not visible -- pass through.
    if (iCursorCount == 0 && iCursorVisible == 0.0) return;

    vec2 vu = norm(fragCoord, 1.0);

    int sweepCount = (iCursorCount > 0) ? min(iCursorCount, MAX_CURSORS) : 1;
    for (int ci = 0; ci < sweepCount; ci++) {

    vec4 currentCursor, previousCursor;
    vec2 cellSize;
    if (iCursorCount > 0) {
        currentCursor = vec4(norm(iCursors[ci].xy, 1.0), norm(iCursors[ci].zw, 0.0));
        previousCursor = vec4(norm(iPreviousCursors[ci].xy, 1.0), norm(iPreviousCursors[ci].zw, 0.0));
        cellSize = currentCursor.zw;  // multi-cursor .zw is already cell-sized
    } else {
        currentCursor = vec4(norm(iCurrentCursor.xy, 1.0), norm(iCurrentCursor.zw, 0.0));
        previousCursor = vec4(norm(iPreviousCursor.xy, 1.0), norm(iPreviousCursor.zw, 0.0));
        cellSize = norm(iCellSize, 0.0);  // cell-based coords for movement detection
    }

    vec2 centerCC = cellCenter(currentCursor.xy, cellSize);
    vec2 effectSizeCC = USE_CELL_SHAPE ? cellSize : currentCursor.zw;
    vec2 halfSizeCC = effectSizeCC * 0.5;
    vec2 centerCP = cellCenter(previousCursor.xy, cellSize);
    vec2 effectSizeCP = USE_CELL_SHAPE ? cellSize : previousCursor.zw;
    vec2 halfSizeCP = effectSizeCP * 0.5;

    float sdfCurrentCursor = sdfRect(vu, centerCC, halfSizeCC);

    float lineLength = distance(centerCC, centerCP);
    float minDist = cellSize.y * THRESHOLD_MIN_DISTANCE;

    vec4 newColor = fragColor;
    float baseProgress = iTime - iTimeCursorChange;

    if (lineLength > minDist && baseProgress < DURATION - 0.001) {

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

        float cc_hh = effectSizeCC.y * 0.5;
        float cc_cy = centerCC.y;
        float cc_nhh = cc_hh * TRAIL_THICKNESS;
        float cc_hw = effectSizeCC.x * 0.5;
        float cc_cx = centerCC.x;
        float cc_nhw = cc_hw * TRAIL_THICKNESS_X;

        vec2 cc_tl = vec2(cc_cx - cc_nhw, cc_cy + cc_nhh);
        vec2 cc_tr = vec2(cc_cx + cc_nhw, cc_cy + cc_nhh);
        vec2 cc_bl = vec2(cc_cx - cc_nhw, cc_cy - cc_nhh);
        vec2 cc_br = vec2(cc_cx + cc_nhw, cc_cy - cc_nhh);

        float cp_hh = effectSizeCP.y * 0.5;
        float cp_cy = centerCP.y;
        float cp_nhh = cp_hh * TRAIL_THICKNESS;
        float cp_hw = effectSizeCP.x * 0.5;
        float cp_cx = centerCP.x;
        float cp_nhw = cp_hw * TRAIL_THICKNESS_X;

        vec2 cp_tl = vec2(cp_cx - cp_nhw, cp_cy + cp_nhh);
        vec2 cp_tr = vec2(cp_cx + cp_nhw, cp_cy + cp_nhh);
        vec2 cp_bl = vec2(cp_cx - cp_nhw, cp_cy - cp_nhh);
        vec2 cp_br = vec2(cp_cx + cp_nhw, cp_cy - cp_nhh);

        const float DURATION_TRAIL = DURATION;
        const float DURATION_LEAD = DURATION * (1.0 - TRAIL_SIZE);
        const float DURATION_SIDE = (DURATION_LEAD + DURATION_TRAIL) / 2.0;

        vec2 moveVec = centerCC - centerCP;
        vec2 s = sign(moveVec);

        float dot_tl = dot(vec2(-1.0, 1.0), s);
        float dot_tr = dot(vec2(1.0, 1.0), s);
        float dot_bl = dot(vec2(-1.0, -1.0), s);
        float dot_br = dot(vec2(1.0, -1.0), s);

        float dur_tl = getDurationFromDot(dot_tl, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);
        float dur_tr = getDurationFromDot(dot_tr, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);
        float dur_bl = getDurationFromDot(dot_bl, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);
        float dur_br = getDurationFromDot(dot_br, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);

        float isMovingRight = step(0.5, s.x);
        float isMovingLeft  = step(0.5, -s.x);

        float dot_right_edge = (dot_tr + dot_br) * 0.5;
        float dur_right_rail = getDurationFromDot(dot_right_edge, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);
        float dot_left_edge = (dot_tl + dot_bl) * 0.5;
        float dur_left_rail = getDurationFromDot(dot_left_edge, DURATION_LEAD, DURATION_SIDE, DURATION_TRAIL);

        float final_dur_tl = mix(dur_tl, dur_left_rail, isMovingLeft);
        float final_dur_bl = mix(dur_bl, dur_left_rail, isMovingLeft);
        float final_dur_tr = mix(dur_tr, dur_right_rail, isMovingRight);
        float final_dur_br = mix(dur_br, dur_right_rail, isMovingRight);

        float prog_tl = ease(clamp(baseProgress / final_dur_tl, 0.0, 1.0));
        float prog_tr = ease(clamp(baseProgress / final_dur_tr, 0.0, 1.0));
        float prog_bl = ease(clamp(baseProgress / final_dur_bl, 0.0, 1.0));
        float prog_br = ease(clamp(baseProgress / final_dur_br, 0.0, 1.0));

        vec2 v_tl = mix(cp_tl, cc_tl, prog_tl);
        vec2 v_tr = mix(cp_tr, cc_tr, prog_tr);
        vec2 v_br = mix(cp_br, cc_br, prog_br);
        vec2 v_bl = mix(cp_bl, cc_bl, prog_bl);

        float sdfTrail = sdfQuad(vu, v_tl, v_tr, v_br, v_bl);

        vec4 trail = TRAIL_COLOR;

        float effectiveBlur = BLUR;
        if (BLUR < 2.5) {
            float isDiagonal = abs(s.x) * abs(s.y);
            effectiveBlur = mix(0.0, BLUR, isDiagonal);
        }
        float shapeAlpha = antialias(sdfTrail, effectiveBlur);

        if (FADE_ENABLED > 0.5) {
            vec2 fragVec = vu - centerCP;
            float fadeProgress = clamp(dot(fragVec, moveVec) / (dot(moveVec, moveVec) + 1e-6), 0.0, 1.0);
            trail.a *= pow(fadeProgress, FADE_EXPONENT);
        }

        float finalAlpha = trail.a * shapeAlpha;
        newColor = mix(newColor, vec4(trail.rgb, newColor.a), finalAlpha);
        newColor = mix(newColor, fragColor, step(sdfCurrentCursor, 0.0));
    }

    fragColor = vec4(newColor.rgb, fragColor.a);

    } // end sweepCount loop

    if (CURSOR_HOLDOUT) {
        fragColor = cursorHoldout(fragColor, original, fragCoord);
    }
}
