// SPDX-License-Identifier: GPL-3.0-or-later
// The rectangle a video picture is drawn in (see nv12.frag).
//
// Compiled to nv12.vert.qsb with Qt's qsb (see shaders.qrc).
#version 440

layout(location = 0) in vec4 position;
layout(location = 1) in vec2 texcoord;
layout(location = 0) out vec2 coord;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec4 sizes;
    vec4 luma;
    vec4 chroma;
};

out gl_PerVertex { vec4 gl_Position; };

void main() {
    coord = texcoord;
    gl_Position = qt_Matrix * position;
}
