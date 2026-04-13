// Bokeh blur - horizontal pass.
// Pair with viewport_bokeh_v.glsl for full effect.

const float MAX_BLUR = 8.0;
const float MAX_DISTANCE = 0.0;
const float BLUR_POWER = 3.0;

#define TAPS 5

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;

    float dist = distance(fragCoord, iMousePosition);
    float maxDist = MAX_DISTANCE > 0.0 ? MAX_DISTANCE
        : length(max(iMousePosition, iResolution.xy - iMousePosition));

    float t = clamp(dist / maxDist, 0.0, 1.0);
    float eased = 1.0 - pow(1.0 - t, BLUR_POWER);
    float radius = eased * MAX_BLUR;

    if (radius < 0.5) {
        fragColor = texture(iChannel0, uv);
        return;
    }

    float px = 1.0 / iResolution.x;
    float sigma = float(TAPS) / 4.0;
    vec3 total = vec3(0.0, 0.0, 0.0);
    float wTotal = 0.0;

    for (int i = 0; i < TAPS; i++) {
        float x = float(i) - float(TAPS / 2);
        float w = exp(-0.5 * x * x / (sigma * sigma));
        float offset = x * radius / float(TAPS / 2);
        total += texture(iChannel0, uv + vec2(offset * px, 0.0)).rgb * w;
        wTotal += w;
    }

    vec3 blurred = total / wTotal;
    vec4 center = texture(iChannel0, uv);
    fragColor = vec4(mix(center.rgb, blurred, eased), center.a);
}
