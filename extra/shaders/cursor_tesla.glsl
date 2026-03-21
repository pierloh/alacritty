#pragma include "lib.glsl"

// Tesla coil cursor trail effect.
// Original by p424p424 (config_ghostty).

float ease(float x) {
    return pow(1.0 - x, 10.0);
}

float blend(float t) {
    float sqr = t * t;
    return sqr / (2.0 * (sqr - t) + 1.0);
}

float antialising(float distance) {
    return 1. - smoothstep(0., norm(vec2(2., 2.), 0.).x, distance);
}

// Tesla coil effect functions
float random(vec2 st) {
    return fract(sin(dot(st.xy, vec2(12.9898, 78.233))) * 43758.5453123);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    float a = random(i);
    float b = random(i + vec2(1.0, 0.0));
    float c = random(i + vec2(0.0, 1.0));
    float d = random(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(a, b, u.x) + (c - a) * u.y * (1.0 - u.x) + (d - b) * u.x * u.y;
}

float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.5;
    for (int i = 0; i < 4; i++) {
        value += amplitude * noise(p);
        p *= 2.0;
        amplitude *= 0.5;
    }
    return value;
}

float electricArc(vec2 p, vec2 a, vec2 b, float time, float progress) {
    vec2 dir = normalize(b - a);
    vec2 perp = vec2(-dir.y, dir.x);
    float dist = distance(a, b);

    float t = clamp(dot(p - a, dir) / dist, 0.0, 1.0);
    vec2 projected = a + dir * t * dist;
    vec2 offset = perp * (fbm(vec2(t * 10.0, time * 10.0)) - 0.5) * 0.1 * progress;

    float d = length(p - (projected + offset));

    float branch = 0.0;
    if (progress > 0.5) {
        float branchTime = time * 15.0;
        float branchFreq = 5.0;
        vec2 branchOffset = perp * (sin(t * branchFreq + branchTime) * 0.03 * progress);
        branch = length(p - (projected + branchOffset));
        d = min(d, branch);
    }

    return d;
}

const vec4 TRAIL_COLOR = vec4(129.0/255.0, 161.0/255.0, 193.0/255.0, 1.0);
const vec4 CURRENT_CURSOR_COLOR = TRAIL_COLOR;
const vec4 PREVIOUS_CURSOR_COLOR = TRAIL_COLOR;
const vec4 TRAIL_COLOR_ACCENT = vec4(0.705, 0.831, 0.957, 1.0);
const vec4 ELECTRIC_COLOR = vec4(0.5, 0.8, 1.0, 1.0);
const float DURATION = 2.2;
const float OPACITY = .001;
const float TAIL_EXTENSION = 1.5;

// HOLDOUT -- punch out cursor shape from the effect
const bool CURSOR_HOLDOUT = true;          // true = effect doesn't render on top of cursor

// EFFECT SHAPE -- use cell dimensions instead of cursor shape for the visual effect
const bool USE_CELL_SHAPE = false;  // true = cell-sized effect, false = follows cursor shape

const bool SKIP_SINGLE_CELL_MOVE = true;     // Skip effect for single-cell moves (typing, arrow keys)

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 original = texture(iChannel0, fragCoord.xy / iResolution.xy);
    fragColor = original;

    if (iCursorCount == 0 && iCursorVisible == 0.0) {
        return;
    }

    vec2 vu = norm(fragCoord, 1.);

    int sweepCount = (iCursorCount > 0) ? min(iCursorCount, MAX_CURSORS) : 1;
    for (int ci = 0; ci < sweepCount; ci++) {

    vec4 currentCursor, previousCursor;
    vec2 cellSize;
    if (iCursorCount > 0) {
        currentCursor = vec4(norm(iCursors[ci].xy, 1.), norm(iCursors[ci].zw, 0.));
        previousCursor = vec4(norm(iPreviousCursors[ci].xy, 1.), norm(iPreviousCursors[ci].zw, 0.));
        cellSize = currentCursor.zw;  // multi-cursor: zw is already cell-sized
    } else {
        currentCursor = vec4(norm(iCurrentCursor.xy, 1.), norm(iCurrentCursor.zw, 0.));
        previousCursor = vec4(norm(iPreviousCursor.xy, 1.), norm(iPreviousCursor.zw, 0.));
        cellSize = norm(iCellSize, 0.0);
    }

    vec2 centerCC = cellCenter(currentCursor.xy, cellSize);
    vec2 centerCP = cellCenter(previousCursor.xy, cellSize);
    vec2 centerCP_new = centerCP + (centerCP - centerCC) * TAIL_EXTENSION;

    float progress = blend(clamp((iTime - iTimeCursorChange) / DURATION, 0.0, 1));
    float easedProgress = ease(progress);

    float lineLength = distance(centerCC, centerCP_new);

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

    float distanceToEnd = distance(vu.xy, centerCC);
    float alphaModifier = distanceToEnd / (lineLength * easedProgress);
    if (alphaModifier > 1.0) {
        alphaModifier = 1.0;
    }

    float trailOpacity = 1.0 - smoothstep(0.0, 1.0, alphaModifier);

    float arcThickness = 0.005 + 0.003 * sin(iTime * 30.0);
    float arc = electricArc(vu, centerCC, centerCP_new, iTime, easedProgress);
    float arcAlpha = 1.0 - smoothstep(arcThickness * 0.5, arcThickness, arc);

    float glow = 1.0 - smoothstep(arcThickness, arcThickness * 2.0, arc);

    vec4 newColor = fragColor;

    if (arcAlpha > 0.0) {
        vec4 arcColor = mix(ELECTRIC_COLOR, TRAIL_COLOR_ACCENT,
                          0.5 + 0.5 * sin(iTime * 20.0));
        newColor = mix(newColor, arcColor, arcAlpha * trailOpacity);
        newColor = mix(newColor, ELECTRIC_COLOR * 0.5, glow * trailOpacity * 0.3);
    }

    vec2 effectSize = USE_CELL_SHAPE ? cellSize : currentCursor.zw;
    float sdfCursor = sdfRect(vu, cellCenter(currentCursor.xy, effectSize), effectSize * 0.5);
    newColor = mix(newColor, CURRENT_CURSOR_COLOR, 1.0 - smoothstep(0.0, 0.01, sdfCursor));

    fragColor = newColor;

    } // end sweepCount loop

    if (CURSOR_HOLDOUT) {
        fragColor = cursorHoldout(fragColor, original, fragCoord);
    }
}
