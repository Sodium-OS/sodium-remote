// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
#version 440
// Recolours a symbolic icon (Icon.qml): the icon's alpha in `color`.
// Lets icons follow hover tints and the 400 ms re-tint without re-rendering the SVG.

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec4 color;
};

layout(binding = 1) uniform sampler2D source;

void main() {
    float a = texture(source, qt_TexCoord0).a;
    // `color` arrives premultiplied (Qt premultiplies a color uniform), as in glass.frag
    fragColor = color * a * qt_Opacity;
}
