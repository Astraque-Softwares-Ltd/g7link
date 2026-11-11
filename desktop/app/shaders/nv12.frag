// SPDX-License-Identifier: GPL-3.0-or-later
// A decoded video picture (NV12) drawn as it is, so no thread turns every
// picture into RGB first. The two planes come as plain bytes in RGBA
// textures, four samples to a texel: the Y plane's row of bytes, and the UV
// plane's row of U, V pairs. The right byte is picked out here and neighbors
// are blended by hand, as the GPU's own filtering would mix unrelated bytes.
//
// Compiled to nv12.frag.qsb with Qt's qsb (see shaders.qrc).
#version 440

layout(location = 0) in vec2 coord;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    // The picture's width and height in pixels, then its textures' width in
    // texels (both planes: a quarter of the padded row).
    vec4 sizes;
    // Y's offset and scale, for limited or full range.
    vec4 luma;
    // What V adds to red, U and V to green, and U to blue.
    vec4 chroma;
};

layout(binding = 1) uniform sampler2D lumaPlane;
layout(binding = 2) uniform sampler2D chromaPlane;

// The Y sample of the pixel at `p` (whole numbers).
float lumaAt(vec2 p) {
    p = clamp(p, vec2(0.0), sizes.xy - 1.0);
    float texel = floor(p.x / 4.0);
    vec4 four = texture(lumaPlane, (vec2(texel, p.y) + 0.5) / vec2(sizes.z, sizes.y));
    float which = p.x - texel * 4.0;
    return which < 0.5 ? four.r : (which < 1.5 ? four.g : (which < 2.5 ? four.b : four.a));
}

// The U and V samples at `c`, in the half-size chroma plane (whole numbers).
vec2 chromaAt(vec2 c) {
    vec2 half_ = floor(sizes.xy / 2.0);
    c = clamp(c, vec2(0.0), half_ - 1.0);
    float texel = floor(c.x / 2.0);
    vec4 two = texture(chromaPlane, (vec2(texel, c.y) + 0.5) / vec2(sizes.z, half_.y));
    return c.x - texel * 2.0 < 0.5 ? two.rg : two.ba;
}

void main() {
    vec2 p = coord * sizes.xy - 0.5;
    vec2 corner = floor(p);
    vec2 part = p - corner;
    float y = mix(mix(lumaAt(corner), lumaAt(corner + vec2(1.0, 0.0)), part.x),
                  mix(lumaAt(corner + vec2(0.0, 1.0)), lumaAt(corner + vec2(1.0, 1.0)), part.x), part.y);

    vec2 c = coord * sizes.xy * 0.5 - 0.5;
    corner = floor(c);
    part = c - corner;
    vec2 uv = mix(mix(chromaAt(corner), chromaAt(corner + vec2(1.0, 0.0)), part.x),
                  mix(chromaAt(corner + vec2(0.0, 1.0)), chromaAt(corner + vec2(1.0, 1.0)), part.x), part.y);

    y = (y - luma.x) * luma.y;
    uv -= 128.0 / 255.0;
    vec3 rgb = vec3(y + chroma.x * uv.y, y + chroma.y * uv.x + chroma.z * uv.y, y + chroma.w * uv.x);
    fragColor = vec4(clamp(rgb, 0.0, 1.0), 1.0) * qt_Opacity;
}
