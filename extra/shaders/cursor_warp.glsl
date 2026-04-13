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
    if (iCurrentCursorCount == 0) return;

    vec2 vu = norm(fragCoord, 1.0);

    int cursorCount = min(iCurrentCursorCount, MAX_CURSORS);
    for (int ci = 0; ci < cursorCount; ci++) {

    vec4 currentCursor = vec4(norm(iCurrentCursors[ci].xy, 1.0), norm(iCurrentCursors[ci].zw, 0.0));
    vec4 previousCursor = vec4(norm(iPreviousCursors[ci].xy, 1.0), norm(iPreviousCursors[ci].zw, 0.0));

    vec2 currentCenter = cursorCenter(currentCursor.xy, currentCursor.zw);
    vec2 currentEffectSize = currentCursor.zw;
    vec2 currentHalf = currentEffectSize * 0.5;
    vec2 previousCenter = cursorCenter(previousCursor.xy, previousCursor.zw);
    vec2 previousEffectSize = previousCursor.zw;
    vec2 previousHalf = previousEffectSize * 0.5;

    float sdfCurrent = sdfRect(vu, currentCenter, currentHalf);

    float lineLength = distance(currentCenter, previousCenter);
    float minDist = currentCursor.w * THRESHOLD_MIN_DISTANCE;

    vec4 newColor = fragColor;
    float baseProgress = iTime - iTimeCursorChange;

    if (lineLength > minDist && baseProgress < DURATION - 0.001) {

        float cur_hh = currentEffectSize.y * 0.5;
        float cur_cy = currentCenter.y;
        float cur_nhh = cur_hh * TRAIL_THICKNESS;
        float cur_hw = currentEffectSize.x * 0.5;
        float cur_cx = currentCenter.x;
        float cur_nhw = cur_hw * TRAIL_THICKNESS_X;

        vec2 cur_tl = vec2(cur_cx - cur_nhw, cur_cy + cur_nhh);
        vec2 cur_tr = vec2(cur_cx + cur_nhw, cur_cy + cur_nhh);
        vec2 cur_bl = vec2(cur_cx - cur_nhw, cur_cy - cur_nhh);
        vec2 cur_br = vec2(cur_cx + cur_nhw, cur_cy - cur_nhh);

        float prev_hh = previousEffectSize.y * 0.5;
        float prev_cy = previousCenter.y;
        float prev_nhh = prev_hh * TRAIL_THICKNESS;
        float prev_hw = previousEffectSize.x * 0.5;
        float prev_cx = previousCenter.x;
        float prev_nhw = prev_hw * TRAIL_THICKNESS_X;

        vec2 prev_tl = vec2(prev_cx - prev_nhw, prev_cy + prev_nhh);
        vec2 prev_tr = vec2(prev_cx + prev_nhw, prev_cy + prev_nhh);
        vec2 prev_bl = vec2(prev_cx - prev_nhw, prev_cy - prev_nhh);
        vec2 prev_br = vec2(prev_cx + prev_nhw, prev_cy - prev_nhh);

        const float DURATION_TRAIL = DURATION;
        const float DURATION_LEAD = DURATION * (1.0 - TRAIL_SIZE);
        const float DURATION_SIDE = (DURATION_LEAD + DURATION_TRAIL) / 2.0;

        vec2 moveVec = currentCenter - previousCenter;
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

        vec2 v_tl = mix(prev_tl, cur_tl, prog_tl);
        vec2 v_tr = mix(prev_tr, cur_tr, prog_tr);
        vec2 v_br = mix(prev_br, cur_br, prog_br);
        vec2 v_bl = mix(prev_bl, cur_bl, prog_bl);

        float sdfTrail = sdfQuad(vu, v_tl, v_tr, v_br, v_bl);

        vec4 trail = TRAIL_COLOR;

        float effectiveBlur = BLUR;
        if (BLUR < 2.5) {
            float isDiagonal = abs(s.x) * abs(s.y);
            effectiveBlur = mix(0.0, BLUR, isDiagonal);
        }
        float shapeAlpha = antialias(sdfTrail, effectiveBlur);

        if (FADE_ENABLED > 0.5) {
            vec2 fragVec = vu - previousCenter;
            float fadeProgress = clamp(dot(fragVec, moveVec) / (dot(moveVec, moveVec) + 1e-6), 0.0, 1.0);
            trail.a *= pow(fadeProgress, FADE_EXPONENT);
        }

        float finalAlpha = trail.a * shapeAlpha;
        newColor = mix(newColor, vec4(trail.rgb, newColor.a), finalAlpha);
        newColor = mix(newColor, fragColor, step(sdfCurrent, 0.0));
    }

    fragColor = vec4(newColor.rgb, fragColor.a);

    } // end cursor loop

    if (CURSOR_HOLDOUT) {
        fragColor = cursorHoldout(fragColor, original, fragCoord);
    }
}
