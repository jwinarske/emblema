#version 300 es

precision highp float;
precision highp int;

struct Paint {
    vec4 stops[4];
    vec4 offsets;
    vec4 geometry;
    vec4 to_local[3];
    vec4 params;
    vec4 recolor[4];
    vec4 filter_offset;
    vec4 filter_params;
};
struct VertexOutput {
    vec4 position;
    vec3 clip;
    vec2 uv;
    vec4 tint;
};
layout(std140) uniform Paint_block_0Fragment { Paint _group_1_binding_0_fs; };

uniform highp sampler2D _group_0_binding_0_fs;

smooth in vec3 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
smooth in vec4 _vs2fs_location2;
layout(location = 0) out vec4 _fs2p_location0;

void main() {
    VertexOutput in_ = VertexOutput(gl_FragCoord, _vs2fs_location0, _vs2fs_location1, _vs2fs_location2);
    vec3 clip = vec3(0.0);
    clip = in_.clip;
    vec3 _e3 = clip;
    float _e6 = clip.z;
    vec2 coord = (((_e3.xy / vec2(_e6)) * 0.5) + vec2(0.5));
    vec4 texel = textureLod(_group_0_binding_0_fs, vec2(coord), 0.0);
    vec4 tint_1 = _group_1_binding_0_fs.stops[0];
    vec4 color = (texel * tint_1);
    _fs2p_location0 = vec4((color.xyz * color.w), color.w);
    return;
}

