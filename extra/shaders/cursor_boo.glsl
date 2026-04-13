#pragma include "lib.glsl"

// --- CONFIGURATION ---
vec4 TRAIL_COLOR = iCurrentCursorColor; // can change to eg: vec4(0.2, 0.6, 1.0, 0.5);

// === MASTER ANIMATION PARAMETERS ===
const float SMEAR_ENABLED = 1.0;        // 1.0 = enable smear trail, 0.0 = disable
const float ANIMATION_LENGTH = 0.2;     // MASTER: total animation duration per leg (your tuned value)
const float TRAIL_SIZE = 1.0;           // 0.0 = no trail, 1.0 = maximum trail length

// === EDGE LAG PARAMETERS ===
// KEY: Lead edge catches up VERY FAST, trail edge lags fully
const float LEAD_EDGE_LAG = 0.05;       // 0.05 = lead catches up 20× faster than trail (your tuned value)
const float TRAIL_EDGE_LAG = 1.0;       // 1.0 = trail edge lags fully (maximum stretch)

// === EASING PARAMETERS ===
const float EASE_POWER = 2.0;           // Controls easing curve (2.0 = smooth, higher = more extreme)

// === ALPHA GRADIENT PARAMETERS ===
// 100% alpha at lead (cursor), 0% alpha at trail (lagging end)
const float SMEAR_BASE_ALPHA = 0.85;    // Base opacity multiplier for the trail
const float LEAD_EDGE_ALPHA = 1.0;      // Alpha at leading edge (cursor position) - fully opaque
const float TRAIL_EDGE_ALPHA = 0.0;     // Alpha at trailing edge (lagging end) - transparent

// === LEG COMPLETION (TOUR QUEUING SIMULATION) ===
// Maximize persistence so multiple legs remain visible (creates "tour" illusion)
const float SEGMENT_FADE_HOLD = 0.95;   // Start fade at 95% (slightly earlier for smoother transition)
const float LEG_PERSISTENCE = 0.3;      // Extra seconds leg remains visible after completion (MAXIMIZED)

// === MOVEMENT VALIDATION ===
// REVERTED: Allow all legitimate movements; scroll glitch is engine-level limitation
const float MAX_VALID_MOVE_DISTANCE = 100.0;  // Maximum cursor heights for valid leg (restored)
const float MIN_MOVE_DISTANCE = 0.0;          // Minimum movement to trigger smear

// === RENDERING PARAMETERS ===
const float BLUR = 2.0;
const float DURATION = 0.2;
const float THRESHOLD_MIN_DISTANCE = 0.0;

// Original warp trail parameters (fallback)
const float TRAIL_THICKNESS = 1.0;
const float TRAIL_THICKNESS_X = 0.9;
const float FADE_ENABLED = 0.0;
const float FADE_EXPONENT = 5.0;

// HOLDOUT -- punch out cursor shape from the effect
const bool CURSOR_HOLDOUT = true;          // true = effect doesn't render on top of cursor

// --- CONSTANTS ---
const float PI = 3.14159265359;

// --- EASING FUNCTIONS ---

// Ease-in-out: slow start → fast middle → slow end
float easeInOut(float t) {
    t = clamp(t, 0.0, 1.0);
    if (t < 0.5) {
        return 0.5 * pow(2.0 * t, EASE_POWER);
    } else {
        return 1.0 - 0.5 * pow(2.0 * (1.0 - t), EASE_POWER);
    }
}

// // Ease-out (alternative - uncomment to use later)
// float easeOut(float t) {
//     t = clamp(t, 0.0, 1.0);
//     return 1.0 - pow(1.0 - t, EASE_POWER);
// }

// // Linear (alternative - uncomment to use later)
// float easeLinear(float t) {
//     return clamp(t, 0.0, 1.0);
// }

// Get cursor corner positions from center and half-size
vec2 getTopLeft(vec2 center, vec2 halfSize) {
    return center + vec2(-halfSize.x, halfSize.y);
}

vec2 getTopRight(vec2 center, vec2 halfSize) {
    return center + vec2(halfSize.x, halfSize.y);
}

vec2 getBottomLeft(vec2 center, vec2 halfSize) {
    return center + vec2(-halfSize.x, -halfSize.y);
}

vec2 getBottomRight(vec2 center, vec2 halfSize) {
    return center + vec2(halfSize.x, -halfSize.y);
}


float antialising(float distance, float blurAmount) {
  return 1. - smoothstep(0., norm(vec2(blurAmount, blurAmount), 0.).x, distance);
}

// Determines animation duration based on a corner's alignment with the move direction (dot product)
float getDurationFromDot(float dot_val, float DURATION_LEAD, float DURATION_SIDE, float DURATION_TRAIL) {
    float isLead = step(0.5, dot_val);
    float isSide = step(-0.5, dot_val) * (1.0 - isLead);
    float duration = mix(DURATION_TRAIL, DURATION_SIDE, isSide);
    duration = mix(duration, DURATION_LEAD, isLead);
    return duration;
}

// Alpha gradient: 100% at lead (cursor) → 0% at trail (lagging end)
float calculateSmearAlphaGradient(in vec2 fragPos, in vec2 trailEdge, in vec2 leadEdge, in vec2 moveDir) {
    float legLength = distance(leadEdge, trailEdge);
    if (legLength < 0.001) {
        return LEAD_EDGE_ALPHA;
    }
    
    // Project fragment onto leg axis (trail → lead)
    vec2 trailToFrag = fragPos - trailEdge;
    float positionAlongLeg = dot(trailToFrag, moveDir) / legLength;
    positionAlongLeg = clamp(positionAlongLeg, 0.0, 1.0);
    
    // === Gradient: 100% at lead (position=1.0), 0% at trail (position=0.0) ===
    float gradientAlpha = positionAlongLeg;
    
    // Soft fade at trail edge (first 10% of leg)
    if (positionAlongLeg < 0.1) {
        gradientAlpha *= smoothstep(0.0, 0.1, positionAlongLeg);
    }
    
    // Ensure lead edge stays at full alpha (last 15% of leg)
    if (positionAlongLeg > 0.85) {
        float blendT = (positionAlongLeg - 0.85) / 0.15;
        gradientAlpha = mix(gradientAlpha, LEAD_EDGE_ALPHA, blendT);
    }
    
    return gradientAlpha;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord){
    #if !defined(WEB)
    vec4 original = texture(iChannel0, fragCoord.xy / iResolution.xy);
    fragColor = original;
    #endif

    if (iCurrentCursorCount == 0) return;

    vec2 vu = norm(fragCoord, 1.);

    int cursorCount = min(iCurrentCursorCount, MAX_CURSORS);
    for (int ci = 0; ci < cursorCount; ci++) {

    vec4 currentCursor = vec4(norm(iCurrentCursors[ci].xy, 1.), norm(iCurrentCursors[ci].zw, 0.));
    vec4 previousCursor = vec4(norm(iPreviousCursors[ci].xy, 1.), norm(iPreviousCursors[ci].zw, 0.));

    vec2 currentCenter = cursorCenter(currentCursor.xy, currentCursor.zw);
    vec2 currentEffectSize = currentCursor.zw;
    vec2 currentHalf = currentEffectSize * 0.5;
    vec2 previousCenter = cursorCenter(previousCursor.xy, previousCursor.zw);
    vec2 previousEffectSize = previousCursor.zw;
    vec2 previousHalf = previousEffectSize * 0.5;

    float sdfCurrent = sdfRect(vu, currentCenter, currentHalf);

    float lineLength = distance(currentCenter, previousCenter);
    float minDist = currentCursor.w * THRESHOLD_MIN_DISTANCE;
    float maxDist = currentCursor.w * MAX_VALID_MOVE_DISTANCE;

    vec4 newColor = vec4(fragColor);
    float baseProgress = iTime - iTimeCursorChange;

    // === Leg validation: render if movement is valid and leg not fully complete ===
    // Extended persistence (ANIMATION_LENGTH + LEG_PERSISTENCE) creates "tour" overlap
    bool isValidMovement = (lineLength > minDist) &&
                           (lineLength < maxDist) &&
                           (baseProgress < (ANIMATION_LENGTH * SEGMENT_FADE_HOLD) + LEG_PERSISTENCE);

    bool animationActive = (baseProgress < ANIMATION_LENGTH - 0.001);

    if (isValidMovement) {

        if (SMEAR_ENABLED > 0.5) {
            vec2 moveVec = currentCenter - previousCenter;
            float moveLength = length(moveVec);
            
            if (moveLength > 0.0001) {
                vec2 moveDir = normalize(moveVec);
                
                // === Normalized progress for this leg (0.0 to 1.0) ===
                float t = clamp(baseProgress / ANIMATION_LENGTH, 0.0, 1.0);
                float easedT = easeInOut(t);
                
                // === Get FIXED corner positions for this leg ===
                vec2 tl_beg = getTopLeft(previousCenter, previousHalf);
                vec2 tr_beg = getTopRight(previousCenter, previousHalf);
                vec2 bl_beg = getBottomLeft(previousCenter, previousHalf);
                vec2 br_beg = getBottomRight(previousCenter, previousHalf);
                
                vec2 tl_end = getTopLeft(currentCenter, currentHalf);
                vec2 tr_end = getTopRight(currentCenter, currentHalf);
                vec2 bl_end = getBottomLeft(currentCenter, currentHalf);
                vec2 br_end = getBottomRight(currentCenter, currentHalf);
                
                // === Detect primary movement direction ===
                float absX = abs(moveVec.x);
                float absY = abs(moveVec.y);
                bool isHorizontal = absX >= absY;
                
                // === Edge lag calculation ===
                // Lead edge catches up VERY FAST (0.05), trail edge lags fully (1.0)
                float leadProgress = clamp(easedT / LEAD_EDGE_LAG, 0.0, 1.0);  // Very fast catch-up
                float trailProgress = clamp(easedT / TRAIL_EDGE_LAG, 0.0, 1.0);  // Normal lag
                
                vec2 q_tl, q_tr, q_br, q_bl;
                
                if (isHorizontal) {
                    if (moveVec.x > 0.0) {
                        // Moving RIGHT: left corners trail, right corners lead
                        q_tl = mix(tl_beg, tl_end, trailProgress * TRAIL_SIZE);
                        q_bl = mix(bl_beg, bl_end, trailProgress * TRAIL_SIZE);
                        q_tr = mix(tr_beg, tr_end, leadProgress * TRAIL_SIZE);
                        q_br = mix(br_beg, br_end, leadProgress * TRAIL_SIZE);
                    } else {
                        // Moving LEFT: right corners trail, left corners lead
                        q_tr = mix(tr_beg, tr_end, trailProgress * TRAIL_SIZE);
                        q_br = mix(br_beg, br_end, trailProgress * TRAIL_SIZE);
                        q_tl = mix(tl_beg, tl_end, leadProgress * TRAIL_SIZE);
                        q_bl = mix(bl_beg, bl_end, leadProgress * TRAIL_SIZE);
                    }
                } else {
                    if (moveVec.y > 0.0) {
                        // Moving UP: bottom corners trail, top corners lead
                        q_bl = mix(bl_beg, bl_end, trailProgress * TRAIL_SIZE);
                        q_br = mix(br_beg, br_end, trailProgress * TRAIL_SIZE);
                        q_tl = mix(tl_beg, tl_end, leadProgress * TRAIL_SIZE);
                        q_tr = mix(tr_beg, tr_end, leadProgress * TRAIL_SIZE);
                    } else {
                        // Moving DOWN: top corners trail, bottom corners lead
                        q_tl = mix(tl_beg, tl_end, trailProgress * TRAIL_SIZE);
                        q_tr = mix(tr_beg, tr_end, trailProgress * TRAIL_SIZE);
                        q_bl = mix(bl_beg, bl_end, leadProgress * TRAIL_SIZE);
                        q_br = mix(br_beg, br_end, leadProgress * TRAIL_SIZE);
                    }
                }
                
                // === Build trail quad SDF (CCW order: TL → TR → BR → BL) ===
                float sdfTrail = sdfQuad(vu, q_tl, q_tr, q_br, q_bl);
                
                // === Calculate actual trail and lead edge positions for alpha gradient ===
                vec2 trailEdgePos, leadEdgePos;
                if (isHorizontal) {
                    if (moveVec.x > 0.0) {
                        trailEdgePos = mix(tl_beg, tl_end, trailProgress * TRAIL_SIZE);
                        leadEdgePos = mix(tr_beg, tr_end, leadProgress * TRAIL_SIZE);
                    } else {
                        trailEdgePos = mix(tr_beg, tr_end, trailProgress * TRAIL_SIZE);
                        leadEdgePos = mix(tl_beg, tl_end, leadProgress * TRAIL_SIZE);
                    }
                } else {
                    if (moveVec.y > 0.0) {
                        trailEdgePos = mix(bl_beg, bl_end, trailProgress * TRAIL_SIZE);
                        leadEdgePos = mix(tl_beg, tl_end, leadProgress * TRAIL_SIZE);
                    } else {
                        trailEdgePos = mix(tl_beg, tl_end, trailProgress * TRAIL_SIZE);
                        leadEdgePos = mix(bl_beg, bl_end, leadProgress * TRAIL_SIZE);
                    }
                }
                vec2 legDir = normalize(leadEdgePos - trailEdgePos);
                
                // === Alpha gradient: 100% at lead (cursor) → 0% at trail ===
                float gradientAlpha = calculateSmearAlphaGradient(vu, trailEdgePos, leadEdgePos, legDir);
                float smearAlpha = SMEAR_BASE_ALPHA * gradientAlpha;
                
                // === Temporal fade: gentle in, held until leg nearly complete ===
                float temporalFade = 1.0;
                if (t < 0.05) {
                    temporalFade *= smoothstep(0.0, 0.05, t);
                }
                if (t > SEGMENT_FADE_HOLD) {
                    temporalFade *= smoothstep(1.0, SEGMENT_FADE_HOLD, t);
                }
                smearAlpha *= temporalFade;
                
                // Antialiasing
                float effectiveBlur = BLUR;
                if (BLUR < 2.5) {
                    float isDiagonal = step(0.5, absX) * step(0.5, absY);
                    effectiveBlur = mix(1.0, BLUR, isDiagonal);
                }
                float shapeAlpha = antialising(sdfTrail, effectiveBlur);
                
                // Composite trail
                if (smearAlpha * shapeAlpha > 0.001) {
                    vec4 smearColor = TRAIL_COLOR;
                    smearColor.a = smearAlpha * shapeAlpha;
                    newColor = mix(newColor, vec4(smearColor.rgb, newColor.a), smearColor.a);
                }
                
                // Keep cursor opaque during animation (no blink hole)
                if (animationActive) {
                    float cursorAlpha = step(sdfCurrent, 0.0);
                    newColor = mix(newColor, vec4(TRAIL_COLOR.rgb, 1.0), cursorAlpha);
                } else {
                    newColor = mix(newColor, fragColor, step(sdfCurrent, 0.0));
                }
            }
        } 
        // === ORIGINAL WARP TRAIL MODE (fallback) ===
        else {
            float cur_half_height = currentEffectSize.y * 0.5;
            float cur_center_y = currentCenter.y;
            float cur_new_half_height = cur_half_height * TRAIL_THICKNESS;
            float cur_new_top_y = cur_center_y + cur_new_half_height;
            float cur_new_bottom_y = cur_center_y - cur_new_half_height;

            float cur_half_width = currentEffectSize.x * 0.5;
            float cur_center_x = currentCenter.x;
            float cur_new_half_width = cur_half_width * TRAIL_THICKNESS_X;
            float cur_new_left_x = cur_center_x - cur_new_half_width;
            float cur_new_right_x = cur_center_x + cur_new_half_width;

            vec2 cur_tl = vec2(cur_new_left_x, cur_new_top_y);
            vec2 cur_tr = vec2(cur_new_right_x, cur_new_top_y);
            vec2 cur_bl = vec2(cur_new_left_x, cur_new_bottom_y);
            vec2 cur_br = vec2(cur_new_right_x, cur_new_bottom_y);

            float prev_half_height = previousEffectSize.y * 0.5;
            float prev_center_y = previousCenter.y;
            float prev_new_half_height = prev_half_height * TRAIL_THICKNESS;
            float prev_new_top_y = prev_center_y + prev_new_half_height;
            float prev_new_bottom_y = prev_center_y - prev_half_height;

            float prev_half_width = previousEffectSize.x * 0.5;
            float prev_center_x = previousCenter.x;
            float prev_new_half_width = prev_half_width * TRAIL_THICKNESS_X;
            float prev_new_left_x = prev_center_x - prev_new_half_width;
            float prev_new_right_x = prev_center_x + prev_new_half_width;

            vec2 prev_tl = vec2(prev_new_left_x, prev_new_top_y);
            vec2 prev_tr = vec2(prev_new_right_x, prev_new_top_y);
            vec2 prev_bl = vec2(prev_new_left_x, prev_new_bottom_y);
            vec2 prev_br = vec2(prev_new_right_x, prev_new_bottom_y);

            const float DURATION_TRAIL = DURATION;
            const float DURATION_LEAD = DURATION * (1.0 - TRAIL_SIZE);
            const float DURATION_SIDE = (DURATION_LEAD + DURATION_TRAIL) / 2.0;

            vec2 moveVec = currentCenter - previousCenter;
            vec2 s = sign(moveVec);

            float dot_tl = dot(vec2(-1., 1.), s);
            float dot_tr = dot(vec2( 1., 1.), s);
            float dot_bl = dot(vec2(-1.,-1.), s);
            float dot_br = dot(vec2( 1.,-1.), s);

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

            float prog_tl = easeInOut(clamp(baseProgress / final_dur_tl, 0.0, 1.0));
            float prog_tr = easeInOut(clamp(baseProgress / final_dur_tr, 0.0, 1.0));
            float prog_bl = easeInOut(clamp(baseProgress / final_dur_bl, 0.0, 1.0));
            float prog_br = easeInOut(clamp(baseProgress / final_dur_br, 0.0, 1.0));

            vec2 v_tl = mix(prev_tl, cur_tl, prog_tl);
            vec2 v_tr = mix(prev_tr, cur_tr, prog_tr);
            vec2 v_br = mix(prev_br, cur_br, prog_br);
            vec2 v_bl = mix(prev_bl, cur_bl, prog_bl);

            float sdfTrail = sdfQuad(vu, v_tl, v_tr, v_br, v_bl);

            vec2 fragVec = vu - previousCenter;
            float fadeProgress = clamp(dot(fragVec, moveVec) / (dot(moveVec, moveVec) + 1e-6), 0.0, 1.0);

            vec4 trail = TRAIL_COLOR;
            
            float effectiveBlur = BLUR;
            if (BLUR < 2.5) {
              float isDiagonal = abs(s.x) * abs(s.y);
              float effectiveBlur = mix(0.0, BLUR, isDiagonal);
            }
            float shapeAlpha = antialising(sdfTrail, effectiveBlur);

            if (FADE_ENABLED > 0.5) {
                float easedProgress = pow(fadeProgress, FADE_EXPONENT);
                trail.a *= easedProgress;
            }

            float finalAlpha = trail.a * shapeAlpha;
            newColor = mix(newColor, vec4(trail.rgb, newColor.a), finalAlpha);
        }

        if (animationActive) {
            float cursorAlpha = step(sdfCurrent, 0.0);
            newColor = mix(newColor, vec4(TRAIL_COLOR.rgb, 1.0), cursorAlpha);
        } else {
            newColor = mix(newColor, fragColor, step(sdfCurrent, 0.0));
        }

    }

    fragColor = newColor;

    } // end cursor loop

    if (CURSOR_HOLDOUT) {
        fragColor = cursorHoldout(fragColor, original, fragCoord);
    }
}