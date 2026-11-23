// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#version 440
// Graphite Glass surface in one pass (Glass.qml): the drop shadow outside the
// shape only (CSS box-shadow semantics, so it never darkens the translucent
// tint), the tint, a 1 px hairline and the 1 px top highlight just inside it.
// Corners use Hyprland's rounding curve: |x|^p + |y|^p = r^p (p 2 = circle,
// p 4 = the window squircle). Colors arrive straight (not premultiplied).
// Built to glass.frag.qsb by ui/CMakeLists.txt, and with BACKDROP defined to
// glassblur.frag.qsb: the in-window blur, where the tint lies over a blurred,
// saturated copy of the window content behind the surface (Glass.backdrop),
// like the boards' backdrop-filter: blur(30px) saturate(170%).

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec2 itemSize;      // this effect item, logical px
    vec4 body;          // the surface inside it: x, y, w, h
    float radius;
    float power;
    float aa;           // one device pixel in logical px
    float borderWidth;
    vec4 tint;
    vec4 border;
    vec4 highlight;
    vec4 shadowColor;
    vec2 shadowOffset;
    float shadowSigma;
#ifdef BACKDROP
    vec4 backdropRect;  // where the backdrop texture lies in this item, logical px
    float blurSigma;    // CSS blur radius = Gaussian sigma, logical px
    float blurLod;      // mip level the taps read (the texture is 1 texel per logical px)
    float saturation;   // CSS saturate()
#endif
};

#ifdef BACKDROP
layout(binding = 1) uniform sampler2D backdrop;

// A 7 x 7 Gaussian over the prefiltered mip level: wide and smooth at a fixed
// cost of 49 taps, read only inside the surface.
vec4 blurredBackdrop(vec2 p) {
    vec2 uv = (p - backdropRect.xy) / backdropRect.zw;
    float spacing = blurSigma * 0.6;
    vec2 stepUv = vec2(spacing) / backdropRect.zw;
    float k = spacing * spacing / (2.0 * blurSigma * blurSigma);
    vec4 acc = vec4(0.0);
    float wsum = 0.0;
    for (int j = -3; j <= 3; ++j) {
        for (int i = -3; i <= 3; ++i) {
            vec2 o = vec2(float(i), float(j));
            float w = exp(-dot(o, o) * k);
            acc += w * textureLod(backdrop, uv + o * stepUv, blurLod);
            wsum += w;
        }
    }
    vec4 c = acc / wsum;
    float l = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    c.rgb = clamp(mix(vec3(l), c.rgb, saturation), vec3(0.0), vec3(c.a));
    return c;
}
#endif

float sdShape(vec2 p, vec2 halfSize, float r) {
    vec2 q = abs(p) - halfSize + r;
    vec2 m = max(q, 0.0);
    float corner = power == 2.0 ? length(m) : pow(pow(m.x, power) + pow(m.y, power), 1.0 / power);
    return corner + min(max(q.x, q.y), 0.0) - r;
}

float coverage(float d) {
    return clamp(0.5 - d / aa, 0.0, 1.0);
}

// Abramowitz and Stegun 7.1.26
float erfApprox(float x) {
    float s = sign(x);
    x = abs(x);
    float t = 1.0 / (1.0 + 0.3275911 * x);
    float y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-x * x);
    return s * y;
}

vec4 premul(vec4 c, float a) {
    return vec4(c.rgb * c.a * a, c.a * a);
}

vec4 over(vec4 dst, vec4 src) {
    return src + dst * (1.0 - src.a);
}

void main() {
    vec2 p = qt_TexCoord0 * itemSize;
    vec2 halfSize = body.zw * 0.5;
    vec2 c = p - (body.xy + halfSize);
    float r = min(radius, min(halfSize.x, halfSize.y));

    float d = sdShape(c, halfSize, r);
    float inside = coverage(d);

    vec4 col = vec4(0.0);
    if (shadowColor.a > 0.0 && shadowSigma > 0.0) {
        float ds = sdShape(c - shadowOffset, halfSize, r);
        float s = 0.5 * (1.0 - erfApprox(ds / (shadowSigma * 1.41421356)));
        col = premul(shadowColor, s * (1.0 - inside));
    }
#ifdef BACKDROP
    col = over(col, over(blurredBackdrop(p) * inside, premul(tint, inside)));
#else
    col = over(col, premul(tint, inside));
#endif

    float inner = inside;
    if (border.a > 0.0 && borderWidth > 0.0) {
        inner = coverage(d + borderWidth);
        col = over(col, premul(border, max(inside - inner, 0.0)));
    }
    if (highlight.a > 0.0) {
        // the inner shape minus itself moved down 1 px: a 1 px band under the top edge
        float bw = border.a > 0.0 ? borderWidth : 0.0;
        float below = coverage(sdShape(c - vec2(0.0, 1.0), halfSize - vec2(bw, bw), max(r - bw, 0.0)));
        col = over(col, premul(highlight, max(inner - below, 0.0)));
    }
    fragColor = col * qt_Opacity;
}
