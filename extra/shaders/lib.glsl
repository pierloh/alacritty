// Shared shader utilities for Alacritty custom post-process shaders.
// Usage: #pragma include "lib.glsl"

// --- Coordinate normalization ---

// Normalize pixel coords to aspect-corrected space centered on screen.
// isPos=1.0 for positions (centers on screen), isPos=0.0 for sizes (no offset).
vec2 norm(vec2 v, float isPos) {
    return (v * 2.0 - iResolution.xy * isPos) / iResolution.y;
}

// --- Easing functions ---
// All take t in [0,1], return eased value in [0,1].

// Ease-out (fast start, slow end)
float easeOutQuad(float t) { return 1.0 - (1.0 - t) * (1.0 - t); }
float easeOutCubic(float t) { return 1.0 - pow(1.0 - t, 3.0); }
float easeOutQuart(float t) { return 1.0 - pow(1.0 - t, 4.0); }
float easeOutQuint(float t) { return 1.0 - pow(1.0 - t, 5.0); }
float easeOutCirc(float t) { return sqrt(1.0 - pow(t - 1.0, 2.0)); }
float easeOutExpo(float t) { return t == 1.0 ? 1.0 : 1.0 - pow(2.0, -10.0 * t); }
float easeOutSine(float t) { return sin(t * 1.5707963); }

float easeOutElastic(float t) {
    const float c4 = 2.0943951;  // 2*PI/3
    return t == 0.0 ? 0.0 : t == 1.0 ? 1.0
        : pow(2.0, -10.0 * t) * sin((t * 10.0 - 0.75) * c4) + 1.0;
}

float easeOutBounce(float t) {
    const float n1 = 7.5625, d1 = 2.75;
    if (t < 1.0 / d1) return n1 * t * t;
    if (t < 2.0 / d1) return n1 * (t -= 1.5 / d1) * t + 0.75;
    if (t < 2.5 / d1) return n1 * (t -= 2.25 / d1) * t + 0.9375;
    return n1 * (t -= 2.625 / d1) * t + 0.984375;
}

float easeOutBack(float t) {
    const float c1 = 1.70158, c3 = c1 + 1.0;
    return 1.0 + c3 * pow(t - 1.0, 3.0) + c1 * pow(t - 1.0, 2.0);
}

// Ease-in (slow start, fast end)
float easeInQuad(float t) { return t * t; }
float easeInCubic(float t) { return t * t * t; }
float easeInQuart(float t) { return t * t * t * t; }
float easeInQuint(float t) { return t * t * t * t * t; }
float easeInExpo(float t) { return t == 0.0 ? 0.0 : pow(2.0, 10.0 * t - 10.0); }
float easeInCirc(float t) { return 1.0 - sqrt(1.0 - pow(t, 2.0)); }
float easeInSine(float t) { return 1.0 - cos(t * 1.5707963); }

// Ease-in-out (slow start and end)
float easeInOutQuad(float t) {
    return t < 0.5 ? 2.0 * t * t : 1.0 - pow(-2.0 * t + 2.0, 2.0) / 2.0;
}
float easeInOutCubic(float t) {
    return t < 0.5 ? 4.0 * t * t * t : 1.0 - pow(-2.0 * t + 2.0, 3.0) / 2.0;
}
float easeInOutQuart(float t) {
    return t < 0.5 ? 8.0 * t * t * t * t : 1.0 - pow(-2.0 * t + 2.0, 4.0) / 2.0;
}

// Linear (identity, useful as a placeholder)
float easeLinear(float t) { return t; }

// --- Pulse / fade functions ---
// Rise then fall over t in [0,1].

float easeOutPulse(float t) { return t * (2.0 - t); }
float smoothstepPulse(float t) { return 4.0 * t * (1.0 - t); }
float sinPulse(float t) { return sin(t * 3.1415926); }
float exponentialDecayPulse(float t) { return exp(-3.0 * t) * sin(t * 3.1415926); }
float powerCurvePulse(float t) { float x = t * 2.0 - 1.0; return 1.0 - x * x; }
float doubleSmoothstepPulse(float t) {
    return smoothstep(0.0, 0.5, t) * (1.0 - smoothstep(0.5, 1.0, t));
}

// --- SDF primitives ---

float sdfRect(vec2 p, vec2 center, vec2 halfSize) {
    vec2 d = abs(p - center) - halfSize;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

// Distance from point p to segment [a, b], updating winding sign s.
float sdfSegment(vec2 p, vec2 a, vec2 b, inout float s, float d) {
    vec2 e = b - a;
    vec2 w = p - a;
    vec2 q = w - e * clamp(dot(w, e) / dot(e, e), 0.0, 1.0);
    d = min(d, dot(q, q));
    bvec3 c = bvec3(p.y >= a.y, p.y < b.y, e.x * w.y > e.y * w.x);
    if (all(c) || all(not(c))) s *= -1.0;
    return d;
}

// Signed distance to a convex quad defined by 4 vertices (CCW order).
float sdfQuad(vec2 p, vec2 v0, vec2 v1, vec2 v2, vec2 v3) {
    float s = 1.0;
    float d = dot(p - v0, p - v0);
    d = sdfSegment(p, v0, v1, s, d);
    d = sdfSegment(p, v1, v2, s, d);
    d = sdfSegment(p, v2, v3, s, d);
    d = sdfSegment(p, v3, v0, s, d);
    return s * sqrt(d);
}

// --- Cursor utilities ---

// Compute cursor center from top-left position and size.
vec2 cursorCenter(vec2 pos, vec2 size) {
    return pos + size * vec2(0.5, -0.5);
}

// Determine which corner leads during diagonal movement.
// Returns 1.0 if top-right leads, -1.0 if bottom-left leads.
float leadingCornerSign(vec2 current, vec2 previous) {
    vec2 delta = current - previous;
    return sign(delta.x * delta.y);
}

// Cursor shape holdout: restore original pixels where the cursor shape is.
// Call at the end of mainImage to punch out the cursor from the effect.
// Uses pixel-space coordinates (fragCoord, not normalized).
vec4 cursorHoldout(vec4 effectColor, vec4 originalColor, vec2 fragCoord) {
    int count = min(iCurrentCursorCount, MAX_CURSORS);
    for (int i = 0; i < count; i++) {
        vec2 center = iCurrentCursors[i].xy + iCurrentCursors[i].zw * vec2(0.5, -0.5);
        if (sdfRect(fragCoord, center, iCurrentCursors[i].zw * 0.5) <= 0.0)
            return originalColor;
    }
    return effectColor;
}
