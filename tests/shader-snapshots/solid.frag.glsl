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

vec4 sample_stops(float t, int count) {
    vec4 result = vec4(0.0);
    int i = 1;
    vec4 _e5 = _group_1_binding_0_fs.stops[0];
    result = _e5;
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            int _e45 = i;
            i = (_e45 + 1);
        }
        loop_init = false;
        int _e9 = i;
        if ((_e9 < count)) {
        } else {
            break;
        }
        {
            int _e13 = i;
            float lower = _group_1_binding_0_fs.offsets[(_e13 - 1)];
            int _e20 = i;
            float upper = _group_1_binding_0_fs.offsets[_e20];
            float span = max((upper - lower), 1e-6);
            float local = clamp(((t - lower) / span), 0.0, 1.0);
            if ((t >= lower)) {
                int _e34 = i;
                vec4 _e38 = _group_1_binding_0_fs.stops[(_e34 - 1)];
                int _e41 = i;
                vec4 _e43 = _group_1_binding_0_fs.stops[_e41];
                result = mix(_e38, _e43, local);
            }
        }
    }
    vec4 _e48 = result;
    return _e48;
}

vec2 tile_gradient(float t_1, float tile) {
    if (((tile > 0.5) && (tile < 1.5))) {
        return vec2((t_1 - floor(t_1)), 1.0);
    }
    if (((tile > 1.5) && (tile < 2.5))) {
        float inside = (((t_1 >= 0.0) && (t_1 <= 1.0)) ? 1.0 : 0.0);
        return vec2(clamp(t_1, 0.0, 1.0), inside);
    }
    if ((tile > 2.5)) {
        return vec2((1.0 - abs((1.0 - (t_1 - (2.0 * floor((t_1 * 0.5))))))), 1.0);
    }
    return vec2(clamp(t_1, 0.0, 1.0), 1.0);
}

vec4 gradient_color(float t_2, int count_1) {
    float at = 0.0;
    if ((count_1 <= 0)) {
        at = t_2;
        float _e7 = at;
        vec4 _e11 = textureLod(_group_0_binding_0_fs, vec2(vec2(_e7, 0.5)), 0.0);
        return _e11;
    }
    vec4 _e12 = sample_stops(t_2, count_1);
    return _e12;
}

vec3 rounded_rect_space(VertexOutput in_1) {
    vec2 uv_1 = vec2(0.0);
    uv_1 = in_1.uv;
    vec2 _e4 = uv_1;
    float _e10 = _group_1_binding_0_fs.geometry.x;
    return ((_e10 > 0.5) ? vec3(_e4, 1.0) : in_1.clip);
}

vec2 to_gradient_space(vec3 clip) {
    vec4 _e4 = _group_1_binding_0_fs.to_local[0];
    vec4 _e11 = _group_1_binding_0_fs.to_local[1];
    vec4 _e19 = _group_1_binding_0_fs.to_local[2];
    vec3 mapped = (((_e4.xyz * clip.x) + (_e11.xyz * clip.y)) + (_e19.xyz * clip.z));
    return (mapped.xy / vec2(max(mapped.z, 1e-6)));
}

vec2 gradient_space(VertexOutput in_2) {
    vec2 uv_2 = vec2(0.0);
    uv_2 = in_2.uv;
    vec2 _e4 = uv_2;
    float _e10 = _group_1_binding_0_fs.geometry.x;
    vec3 source = ((_e10 > 0.5) ? vec3(_e4, 1.0) : in_2.clip);
    vec2 _e14 = to_gradient_space(source);
    return _e14;
}

vec2 snapped(vec2 coord) {
    float _e4 = _group_1_binding_0_fs.params.z;
    float _e10 = _group_1_binding_0_fs.params.z;
    if (((_e4 < 0.5) || (_e10 > 1.5))) {
        return coord;
    }
    vec2 size = vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy));
    return ((floor((coord * size)) + vec2(0.5)) / size);
}

float cubic_weight(float x) {
    float t_5 = abs(x);
    if ((t_5 < 1.0)) {
        float cubic_1 = ((12.0 - (9.0 * 0.33333334)) - (6.0 * 0.33333334));
        float square = ((-18.0 + (12.0 * 0.33333334)) + (6.0 * 0.33333334));
        return ((((((cubic_1 * t_5) + square) * t_5) * t_5) + (6.0 - (2.0 * 0.33333334))) / 6.0);
    }
    if ((t_5 < 2.0)) {
        float cubic_2 = (-(0.33333334) - (6.0 * 0.33333334));
        float square_1 = ((6.0 * 0.33333334) + (30.0 * 0.33333334));
        float linear = ((-12.0 * 0.33333334) - (48.0 * 0.33333334));
        return (((((((cubic_2 * t_5) + square_1) * t_5) + linear) * t_5) + ((8.0 * 0.33333334) + (24.0 * 0.33333334))) / 6.0);
    }
    return 0.0;
}

vec4 cubic(vec2 coord_1, vec2 low, vec2 high) {
    vec4 total = vec4(0.0);
    int j = -1;
    int i_1 = 0;
    vec2 size_1 = vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy));
    vec2 position_1 = ((coord_1 * size_1) - vec2(0.5));
    vec2 base = floor(position_1);
    vec2 offset = (position_1 - base);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            int _e57 = j;
            j = (_e57 + 1);
        }
        loop_init_1 = false;
        int _e17 = j;
        if ((_e17 <= 2)) {
        } else {
            break;
        }
        {
            int _e20 = j;
            float _e24 = cubic_weight((float(_e20) - offset.y));
            i_1 = -1;
            bool loop_init_2 = true;
            while(true) {
                if (!loop_init_2) {
                    int _e54 = i_1;
                    i_1 = (_e54 + 1);
                }
                loop_init_2 = false;
                int _e27 = i_1;
                if ((_e27 <= 2)) {
                } else {
                    break;
                }
                {
                    int _e30 = i_1;
                    float _e34 = cubic_weight((float(_e30) - offset.x));
                    float weight = (_e34 * _e24);
                    int _e36 = i_1;
                    int _e38 = j;
                    vec2 texel_2 = clamp((((base + vec2(float(_e36), float(_e38))) + vec2(0.5)) / size_1), low, high);
                    vec4 _e47 = total;
                    vec4 _e51 = textureLod(_group_0_binding_0_fs, vec2(texel_2), 0.0);
                    total = (_e47 + (_e51 * weight));
                }
            }
        }
    }
    float _e61 = total.w;
    float alpha_1 = clamp(_e61, 0.0, 1.0);
    vec4 _e65 = total;
    return vec4(clamp(_e65.xyz, vec3(0.0), vec3(alpha_1)), alpha_1);
}

float level_of(vec2 texels) {
    vec2 _e1 = dFdx(texels);
    vec2 _e3 = dFdy(texels);
    float per_pixel_1 = max(length(_e1), length(_e3));
    return max(log2(max(per_pixel_1, 1e-6)), 0.0);
}

vec4 sampled(vec2 coord_2, vec2 texels_1, vec2 low_1, vec2 high_1) {
    float _e7 = _group_1_binding_0_fs.params.z;
    if ((_e7 > 2.5)) {
        float _e12 = level_of(texels_1);
        vec4 _e13 = textureLod(_group_0_binding_0_fs, vec2(coord_2), _e12);
        return _e13;
    }
    float _e17 = _group_1_binding_0_fs.params.z;
    if ((_e17 > 1.5)) {
        vec4 _e20 = cubic(coord_2, low_1, high_1);
        return _e20;
    }
    vec2 _e23 = snapped(coord_2);
    vec4 _e25 = textureLod(_group_0_binding_0_fs, vec2(_e23), 0.0);
    return _e25;
}

float tile_axis(float t_3, float tile_1) {
    if (((tile_1 > 0.5) && (tile_1 < 1.5))) {
        return fract(t_3);
    }
    if ((tile_1 > 2.5)) {
        return (1.0 - abs((1.0 - (t_3 - (2.0 * floor((t_3 * 0.5)))))));
    }
    return clamp(t_3, 0.0, 1.0);
}

vec2 tile_uv(vec2 uv_3, vec2 tile_2) {
    float _e4 = tile_axis(uv_3.x, tile_2.x);
    float _e7 = tile_axis(uv_3.y, tile_2.y);
    return vec2(_e4, _e7);
}

bool decaled(vec2 uv_4, vec2 tile_3) {
    bvec2 decal = bvec2(greaterThan(tile_3, vec2(1.5)).x && lessThan(tile_3, vec2(2.5)).x, greaterThan(tile_3, vec2(1.5)).y && lessThan(tile_3, vec2(2.5)).y);
    bvec2 outside = bvec2(lessThan(uv_4, vec2(0.0)).x || greaterThan(uv_4, vec2(1.0)).x, lessThan(uv_4, vec2(0.0)).y || greaterThan(uv_4, vec2(1.0)).y);
    return any(bvec2(decal.x && outside.x, decal.y && outside.y));
}

vec4 sample_mesh(vec2 uv_5) {
    vec4 texel = vec4(0.0);
    vec4 _e3 = _group_1_binding_0_fs.geometry;
    vec2 tile_4 = _e3.yz;
    vec2 _e5 = tile_uv(uv_5, tile_4);
    vec4 _e14 = sampled(_e5, (uv_5 * vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy))), vec2(0.0), vec2(1.0));
    texel = _e14;
    bool _e16 = decaled(uv_5, tile_4);
    if (_e16) {
        texel = vec4(0.0);
    }
    vec4 tint_1 = _group_1_binding_0_fs.stops[0];
    vec4 premultiplied_1 = vec4((tint_1.xyz * tint_1.w), tint_1.w);
    vec4 _e28 = texel;
    float _e33 = _group_1_binding_0_fs.geometry.x;
    return ((_e28 * premultiplied_1) * _e33);
}

vec4 sample_image(vec3 clip_1) {
    vec2 coord_3 = vec2(0.0);
    vec4 texel_1 = vec4(0.0);
    vec2 _e1 = to_gradient_space(clip_1);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 tile_5 = _e4.wy;
    vec2 _e6 = tile_uv(_e1, tile_5);
    coord_3 = _e6;
    vec4 source_1 = _group_1_binding_0_fs.stops[0];
    vec2 _e13 = coord_3;
    coord_3 = (source_1.xy + (_e13 * (source_1.zw - source_1.xy)));
    vec2 half_texel = (vec2(0.5) / vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy)));
    vec2 low_2 = min((source_1.xy + half_texel), (source_1.zw - half_texel));
    vec2 high_2 = max((source_1.xy + half_texel), (source_1.zw - half_texel));
    vec2 _e35 = coord_3;
    coord_3 = clamp(_e35, low_2, high_2);
    vec2 size_2 = vec2(uvec2(textureSize(_group_0_binding_0_fs, 0).xy));
    vec2 texels_2 = ((source_1.xy + (_e1 * (source_1.zw - source_1.xy))) * size_2);
    vec2 _e47 = coord_3;
    vec4 _e48 = sampled(_e47, texels_2, low_2, high_2);
    texel_1 = _e48;
    bool _e50 = decaled(_e1, tile_5);
    if (_e50) {
        texel_1 = vec4(0.0);
    }
    vec4 tint_2 = _group_1_binding_0_fs.stops[1];
    vec4 premultiplied_2 = vec4((tint_2.xyz * tint_2.w), tint_2.w);
    vec4 _e62 = texel_1;
    float _e67 = _group_1_binding_0_fs.geometry.z;
    return ((_e62 * premultiplied_2) * _e67);
}

float coverage_of(float distance_, float per_pixel, float width) {
    float inside_1 = clamp((0.5 - (distance_ / per_pixel)), 0.0, 1.0);
    if ((width <= 0.0)) {
        return inside_1;
    }
    float outer = clamp((0.5 - ((distance_ - (width * 0.5)) / per_pixel)), 0.0, 1.0);
    float inner = clamp((0.5 - ((distance_ + (width * 0.5)) / per_pixel)), 0.0, 1.0);
    return (outer - inner);
}

float rounded_rect_distance(vec2 point, vec2 half_size, float radius) {
    float r = 0.0;
    r = radius;
    float _e6 = r;
    vec2 q = ((abs(point) - half_size) + vec2(_e6));
    return ((min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0)))) - radius);
}

float erf7_(float value) {
    float x_1 = (value * 1.1283792);
    float xx = (x_1 * x_1);
    float series = (x_1 + ((0.24295 + ((0.03395 + (0.0104 * xx)) * xx)) * (x_1 * xx)));
    return (series / sqrt((1.0 + (series * series))));
}

float power_distance(vec2 point_1, float exponent, float exponent_inv) {
    float xp = pow(point_1.x, exponent);
    float yp = pow(point_1.y, exponent);
    return pow((xp + yp), exponent_inv);
}

vec4 rrect_blur_coverage(vec3 clip_2) {
    vec2 _e1 = to_gradient_space(clip_2);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 adjust = _e4.xy;
    float s_inv = _group_1_binding_0_fs.geometry.z;
    float min_edge = _group_1_binding_0_fs.geometry.w;
    float scale = _group_1_binding_0_fs.offsets.x;
    float r1_ = _group_1_binding_0_fs.params.z;
    float exponent_1 = _group_1_binding_0_fs.params.w;
    vec2 centered = abs(_e1);
    vec2 adjusted = (centered - adjust);
    float _e33 = power_distance(max(adjusted, vec2(0.0)), exponent_1, (1.0 / exponent_1));
    float inside_2 = min(max(adjusted.x, adjusted.y), 0.0);
    float distance_1 = ((_e33 + inside_2) - r1_);
    float _e43 = erf7_((s_inv * (min_edge + distance_1)));
    float _e45 = erf7_((s_inv * distance_1));
    float coverage_1 = (scale * (_e43 - _e45));
    vec4 tint_3 = _group_1_binding_0_fs.stops[0];
    float alpha_2 = (tint_3.w * clamp(coverage_1, 0.0, 1.0));
    return vec4((tint_3.xyz * alpha_2), alpha_2);
}

vec4 rounded_rect_coverage(vec3 clip_3) {
    float coverage = 0.0;
    vec2 _e1 = to_gradient_space(clip_3);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 half_size_1 = _e4.zw;
    float _e9 = _group_1_binding_0_fs.params.z;
    float radius_1 = clamp(_e9, 0.0, min(half_size_1.x, half_size_1.y));
    float _e18 = _group_1_binding_0_fs.params.w;
    float stroke_1 = max(_e18, 0.0);
    float half_ = (stroke_1 * 0.5);
    float _e26 = _group_1_binding_0_fs.geometry.y;
    float grown = ((stroke_1 > 0.0) ? _e26 : radius_1);
    float _e32 = rounded_rect_distance(_e1, (half_size_1 + vec2(half_)), grown);
    float _e33 = dFdx(_e32);
    float _e34 = dFdy(_e32);
    vec2 gradient = vec2(_e33, _e34);
    float per_pixel_2 = max(length(gradient), 1e-6);
    coverage = clamp((0.5 - (_e32 / per_pixel_2)), 0.0, 1.0);
    vec2 shrunk = (half_size_1 - vec2(half_));
    if (((stroke_1 > 0.0) && (min(shrunk.x, shrunk.y) > 0.0))) {
        float _e59 = rounded_rect_distance(_e1, shrunk, max((radius_1 - half_), 0.0));
        float _e66 = coverage;
        coverage = (_e66 - clamp((0.5 - (_e59 / per_pixel_2)), 0.0, 1.0));
    }
    vec4 tint_4 = _group_1_binding_0_fs.stops[0];
    float _e73 = coverage;
    float alpha_3 = (tint_4.w * _e73);
    return vec4((tint_4.xyz * alpha_3), alpha_3);
}

vec4 disc_coverage(vec2 point_2, vec2 axes) {
    float stroke = 0.0;
    float implicit = (length((point_2 / axes)) - 1.0);
    float _e6 = dFdx(implicit);
    float _e7 = dFdy(implicit);
    vec2 gradient_1 = vec2(_e6, _e7);
    float per_pixel_3 = max(length(gradient_1), 1e-6);
    float _e15 = _group_1_binding_0_fs.params.w;
    stroke = _e15;
    float _e17 = stroke;
    if ((_e17 > 0.0)) {
        float k1_ = max(length((point_2 / axes)), 1e-6);
        float k2_ = length((point_2 / (axes * axes)));
        float _e27 = stroke;
        stroke = ((_e27 * k2_) / k1_);
    }
    float _e30 = stroke;
    float _e31 = coverage_of(implicit, per_pixel_3, _e30);
    vec4 tint_5 = _group_1_binding_0_fs.stops[0];
    float alpha_4 = (tint_5.w * _e31);
    return vec4((tint_5.xyz * alpha_4), alpha_4);
}

vec4 ellipse_coverage(vec3 clip_4) {
    vec2 _e1 = to_gradient_space(clip_4);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec4 _e9 = disc_coverage(_e1, max(_e4.zw, vec2(1e-6)));
    return _e9;
}

vec4 point_field_coverage(vec2 uv_6) {
    vec4 _e4 = disc_coverage(uv_6, vec2(1.0, 1.0));
    return _e4;
}

vec4 shadow_mesh_color(vec2 uv_7) {
    float x_2 = ((uv_7.x * 4.0) - 2.0);
    float a = abs(x_2);
    float b = (((0.278393 + ((0.230389 + ((0.078108 * a) * a)) * a)) * a) + 1.0);
    float erf = (sign(x_2) * (1.0 - (1.0 / (((b * b) * b) * b))));
    vec4 tint_6 = _group_1_binding_0_fs.stops[0];
    float alpha_5 = ((tint_6.w * (1.0 + erf)) * 0.5);
    return vec4((tint_6.xyz * alpha_5), alpha_5);
}

vec4 blur_along_axis(vec3 clip_5) {
    vec4 total_1 = vec4(0.0);
    float weight_sum = 0.0;
    float i_2 = 0.0;
    vec2 _e1 = to_gradient_space(clip_5);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 step_ = _e4.zw;
    float _e9 = _group_1_binding_0_fs.params.z;
    float sigma = max(_e9, 0.0001);
    float reach = max(((sigma - 0.5) * 1.7320508), 0.0);
    float spread = max((reach / 32.0), 1.0);
    float taps = min(ceil((reach / spread)), 32.0);
    float denominator = (-0.5 / (sigma * sigma));
    i_2 = -(taps);
    while(true) {
        float _e35 = i_2;
        if ((_e35 > taps)) {
            break;
        }
        float _e37 = i_2;
        float offset_1 = (_e37 * spread);
        float weight_1 = exp(((offset_1 * offset_1) * denominator));
        vec2 coord_4 = clamp((_e1 + (step_ * offset_1)), vec2(0.0), vec2(1.0));
        vec4 _e49 = total_1;
        vec4 _e53 = textureLod(_group_0_binding_0_fs, vec2(coord_4), 0.0);
        total_1 = (_e49 + (_e53 * weight_1));
        float _e56 = weight_sum;
        weight_sum = (_e56 + weight_1);
        float _e58 = i_2;
        i_2 = (_e58 + 1.0);
    }
    vec4 _e61 = total_1;
    float _e62 = weight_sum;
    return (_e61 / vec4(max(_e62, 1e-6)));
}

vec4 sample_or_nothing(vec2 uv_8) {
    if ((any(lessThan(uv_8, vec2(0.0))) || any(greaterThan(uv_8, vec2(1.0))))) {
        return vec4(0.0);
    }
    vec4 _e15 = textureLod(_group_0_binding_0_fs, vec2(uv_8), 0.0);
    return _e15;
}

vec4 morphology_along_axis(vec3 clip_6) {
    vec4 best = vec4(0.0);
    float i_3 = 1.0;
    vec2 _e1 = to_gradient_space(clip_6);
    vec4 _e4 = _group_1_binding_0_fs.geometry;
    vec2 step_1 = _e4.zw;
    float taps_1 = _group_1_binding_0_fs.params.z;
    float _e13 = _group_1_binding_0_fs.params.w;
    bool dilate = (_e13 > 0.5);
    vec4 _e16 = sample_or_nothing(_e1);
    best = _e16;
    while(true) {
        float _e20 = i_3;
        if ((_e20 > taps_1)) {
            break;
        }
        float _e22 = i_3;
        vec4 _e25 = sample_or_nothing((_e1 + (step_1 * _e22)));
        float _e26 = i_3;
        vec4 _e29 = sample_or_nothing((_e1 - (step_1 * _e26)));
        if (dilate) {
            vec4 _e30 = best;
            best = max(_e30, max(_e25, _e29));
        } else {
            vec4 _e33 = best;
            best = min(_e33, min(_e25, _e29));
        }
        float _e36 = i_3;
        i_3 = (_e36 + 1.0);
    }
    vec4 _e39 = best;
    return _e39;
}

vec3 linear_to_srgb(vec3 c) {
    vec3 m = abs(c);
    vec3 low_3 = (m * 12.92);
    vec3 high_3 = ((1.055 * pow(m, vec3(0.41666666))) - vec3(0.055));
    return (sign(c) * mix(high_3, low_3, lessThanEqual(m, vec3(0.0031308))));
}

vec3 srgb_to_linear(vec3 c_1) {
    vec3 m_1 = abs(c_1);
    vec3 low_4 = (m_1 / vec3(12.92));
    vec3 high_4 = pow(((m_1 + vec3(0.055)) / vec3(1.055)), vec3(2.4));
    return (sign(c_1) * mix(high_4, low_4, lessThanEqual(m_1, vec3(0.04045))));
}

float sat(vec3 c_2) {
    return (max(c_2.x, max(c_2.y, c_2.z)) - min(c_2.x, min(c_2.y, c_2.z)));
}

vec3 set_sat(vec3 c_3, float s) {
    float low_5 = min(c_3.x, min(c_3.y, c_3.z));
    float high_5 = max(c_3.x, max(c_3.y, c_3.z));
    if ((high_5 <= low_5)) {
        return vec3(0.0);
    }
    return (((c_3 - vec3(low_5)) * s) / vec3((high_5 - low_5)));
}

float lum(vec3 c_4) {
    return dot(c_4, vec3(0.3, 0.59, 0.11));
}

vec3 set_lum(vec3 c_5, float l) {
    vec3 out_1 = vec3(0.0);
    float _e2 = lum(c_5);
    vec3 shifted = (c_5 + vec3((l - _e2)));
    float low_6 = min(shifted.x, min(shifted.y, shifted.z));
    float high_6 = max(shifted.x, max(shifted.y, shifted.z));
    float _e16 = lum(shifted);
    out_1 = shifted;
    if ((low_6 < 0.0)) {
        vec3 _e20 = out_1;
        out_1 = (vec3(_e16) + (((_e20 - vec3(_e16)) * _e16) / vec3(max((_e16 - low_6), 1e-8))));
    }
    if ((high_6 > 1.0)) {
        vec3 _e33 = out_1;
        out_1 = (vec3(_e16) + (((_e33 - vec3(_e16)) * (1.0 - _e16)) / vec3(max((high_6 - _e16), 1e-8))));
    }
    vec3 _e46 = out_1;
    return _e46;
}

vec3 nonseparable_b(int mode, vec3 cb, vec3 cs) {
    switch(mode) {
        case 25: {
            float _e3 = sat(cb);
            vec3 _e4 = set_sat(cs, _e3);
            float _e5 = lum(cb);
            vec3 _e6 = set_lum(_e4, _e5);
            return _e6;
        }
        case 26: {
            float _e7 = sat(cs);
            vec3 _e8 = set_sat(cb, _e7);
            float _e9 = lum(cb);
            vec3 _e10 = set_lum(_e8, _e9);
            return _e10;
        }
        case 27: {
            float _e11 = lum(cb);
            vec3 _e12 = set_lum(cs, _e11);
            return _e12;
        }
        case 28: {
            float _e13 = lum(cs);
            vec3 _e14 = set_lum(cb, _e13);
            return _e14;
        }
        default: {
            return cs;
        }
    }
}

float hard_light(float cb_1, float cs_1) {
    if ((cs_1 <= 0.5)) {
        return (cb_1 * (2.0 * cs_1));
    }
    float d = ((2.0 * cs_1) - 1.0);
    return ((cb_1 + d) - (cb_1 * d));
}

float separable_b(int mode_1, float cb_2, float cs_2) {
    switch(mode_1) {
        case 14: {
            return (cb_2 * cs_2);
        }
        case 15: {
            return ((cb_2 + cs_2) - (cb_2 * cs_2));
        }
        case 16: {
            float _e7 = hard_light(cs_2, cb_2);
            return _e7;
        }
        case 17: {
            return min(cb_2, cs_2);
        }
        case 18: {
            return max(cb_2, cs_2);
        }
        case 19: {
            if ((cb_2 <= 0.0)) {
                return 0.0;
            }
            if ((cs_2 >= 1.0)) {
                return 1.0;
            }
            return min((cb_2 / (1.0 - cs_2)), 1.0);
        }
        case 20: {
            if ((cb_2 >= 1.0)) {
                return 1.0;
            }
            if ((cs_2 <= 0.0)) {
                return 0.0;
            }
            return (1.0 - min(((1.0 - cb_2) / cs_2), 1.0));
        }
        case 21: {
            float _e34 = hard_light(cb_2, cs_2);
            return _e34;
        }
        case 22: {
            float d_1 = ((cb_2 > 0.25) ? inversesqrt(max(cb_2, 1e-8)) : ((((16.0 * cb_2) - 12.0) * cb_2) + 4.0));
            float dd = ((cb_2 > 0.25) ? sqrt(max(cb_2, 0.0)) : (d_1 * cb_2));
            if ((cs_2 <= 0.5)) {
                return (cb_2 - (((1.0 - (2.0 * cs_2)) * cb_2) * (1.0 - cb_2)));
            }
            return (cb_2 + (((2.0 * cs_2) - 1.0) * (dd - cb_2)));
        }
        case 23: {
            return abs((cb_2 - cs_2));
        }
        case 24: {
            return ((cb_2 + cs_2) - ((2.0 * cb_2) * cs_2));
        }
        default: {
            return cs_2;
        }
    }
}

vec4 blend_tint(int mode_2, vec4 src, vec4 dst) {
    float sa = 0.0;
    float da = 0.0;
    vec3 mixed = vec3(0.0);
    sa = src.w;
    da = dst.w;
    switch(mode_2) {
        case 0: {
            return vec4(0.0);
        }
        case 1: {
            return src;
        }
        case 2: {
            return dst;
        }
        case 3: {
            float _e9 = sa;
            return (src + (dst * (1.0 - _e9)));
        }
        case 4: {
            float _e14 = da;
            return (dst + (src * (1.0 - _e14)));
        }
        case 5: {
            float _e19 = da;
            return (src * _e19);
        }
        case 6: {
            float _e21 = sa;
            return (dst * _e21);
        }
        case 7: {
            float _e23 = da;
            return (src * (1.0 - _e23));
        }
        case 8: {
            float _e27 = sa;
            return (dst * (1.0 - _e27));
        }
        case 9: {
            float _e31 = da;
            float _e33 = sa;
            return ((src * _e31) + (dst * (1.0 - _e33)));
        }
        case 10: {
            float _e38 = sa;
            float _e40 = da;
            return ((dst * _e38) + (src * (1.0 - _e40)));
        }
        case 11: {
            float _e45 = da;
            float _e49 = sa;
            return ((src * (1.0 - _e45)) + (dst * (1.0 - _e49)));
        }
        case 12: {
            return min((src + dst), vec4(1.0));
        }
        case 13: {
            return (src * dst);
        }
        default: {
            break;
        }
    }
    float _e60 = sa;
    float _e65 = sa;
    vec3 cs_3 = clamp(((_e65 <= 0.0) ? vec3(0.0) : (src.xyz / vec3(_e60))), vec3(0.0), vec3(1.0));
    float _e75 = da;
    float _e80 = da;
    vec3 cb_3 = clamp(((_e80 <= 0.0) ? vec3(0.0) : (dst.xyz / vec3(_e75))), vec3(0.0), vec3(1.0));
    if ((mode_2 >= 25)) {
        vec3 _e92 = nonseparable_b(mode_2, cb_3, cs_3);
        mixed = _e92;
    } else {
        float _e95 = separable_b(mode_2, cb_3.x, cs_3.x);
        float _e98 = separable_b(mode_2, cb_3.y, cs_3.y);
        float _e101 = separable_b(mode_2, cb_3.z, cs_3.z);
        mixed = vec3(_e95, _e98, _e101);
    }
    float _e103 = sa;
    float _e104 = da;
    float _e109 = sa;
    float _e110 = da;
    vec3 _e112 = mixed;
    float _e115 = sa;
    float _e118 = da;
    vec3 rgb = ((((_e103 * (1.0 - _e104)) * cs_3) + ((_e109 * _e110) * _e112)) + (((1.0 - _e115) * _e118) * cb_3));
    float _e122 = sa;
    float _e123 = da;
    float _e124 = sa;
    return vec4(rgb, (_e122 + (_e123 * (1.0 - _e124))));
}

vec4 filtered(vec4 premultiplied) {
    vec4 color = vec4(0.0);
    vec4 out_2 = vec4(0.0);
    float kind = _group_1_binding_0_fs.filter_params.x;
    if ((kind < 0.5)) {
        return premultiplied;
    }
    if ((kind > 4.5)) {
        float _e13 = _group_1_binding_0_fs.recolor[0].x;
        vec4 _e19 = _group_1_binding_0_fs.filter_offset;
        vec4 _e20 = blend_tint(int((_e13 + 0.5)), _e19, premultiplied);
        return _e20;
    }
    bool straight = (kind > 1.5);
    color = premultiplied;
    if (straight) {
        float _e25 = color.w;
        float alpha_6 = max(_e25, 1e-6);
        vec4 _e28 = color;
        float _e33 = color.w;
        color = vec4((_e28.xyz / vec3(alpha_6)), _e33);
    }
    if ((kind > 2.5)) {
        if ((kind < 3.5)) {
            vec4 _e40 = color;
            vec3 _e42 = linear_to_srgb(_e40.xyz);
            float _e44 = color.w;
            out_2 = vec4(_e42, _e44);
        } else {
            vec4 _e46 = color;
            vec3 _e48 = srgb_to_linear(_e46.xyz);
            float _e50 = color.w;
            out_2 = vec4(_e48, _e50);
        }
    } else {
        vec4 _e55 = _group_1_binding_0_fs.recolor[0];
        float _e57 = color.x;
        vec4 _e62 = _group_1_binding_0_fs.recolor[1];
        float _e64 = color.y;
        vec4 _e70 = _group_1_binding_0_fs.recolor[2];
        float _e72 = color.z;
        vec4 _e78 = _group_1_binding_0_fs.recolor[3];
        float _e80 = color.w;
        vec4 _e85 = _group_1_binding_0_fs.filter_offset;
        out_2 = (((((_e55 * _e57) + (_e62 * _e64)) + (_e70 * _e72)) + (_e78 * _e80)) + _e85);
    }
    if (straight) {
        float _e88 = out_2.w;
        float a_1 = clamp(_e88, 0.0, 1.0);
        vec4 _e92 = out_2;
        return vec4((_e92.xyz * a_1), a_1);
    }
    float _e97 = out_2.w;
    float alpha_7 = clamp(_e97, 0.0, 1.0);
    vec4 _e101 = out_2;
    return vec4(((alpha_7 <= 0.0) ? vec3(0.0) : _e101.xyz), alpha_7);
}

float ordered_dither(vec2 frag) {
    uint x_3 = (uint(frag.x) % 8u);
    uint y = (uint(frag.y) ^ x_3);
    uint m_2 = (((((((y & 1u) << 5u) | ((x_3 & 1u) << 4u)) | ((y & 2u) << 2u)) | ((x_3 & 2u) << 1u)) | ((y & 4u) >> 1u)) | ((x_3 & 4u) >> 2u));
    return ((float(m_2) * 0.015625) - 0.4921875);
}

vec4 dithered(vec4 color_1, vec2 frag_1) {
    float alpha = 0.0;
    float amplitude = _group_1_binding_0_fs.filter_params.z;
    if ((amplitude <= 0.0)) {
        return color_1;
    }
    float _e8 = ordered_dither(frag_1);
    float offset_2 = (_e8 * amplitude);
    alpha = color_1.w;
    float _e15 = alpha;
    return vec4((color_1.xyz + vec3(offset_2)), _e15);
}

vec4 shade(VertexOutput in_3) {
    vec4 color_2 = vec4(0.0);
    float t_4 = 0.0;
    bool covered = false;
    vec4 _e4 = _group_1_binding_0_fs.stops[0];
    color_2 = _e4;
    float kind_1 = _group_1_binding_0_fs.params.y;
    if ((kind_1 < 0.5)) {
        vec4 _e12 = color_2;
        float _e15 = color_2.w;
        float _e18 = color_2.w;
        return vec4((_e12.xyz * _e15), _e18);
    }
    float _e23 = _group_1_binding_0_fs.params.x;
    int count_2 = int(_e23);
    if (((kind_1 > 0.5) && (kind_1 < 1.5))) {
        vec4 _e32 = _group_1_binding_0_fs.geometry;
        vec2 axis = _e32.zw;
        float length_squared = max(dot(axis, axis), 1e-6);
        vec2 _e37 = gradient_space(in_3);
        float t_6 = (dot(_e37, axis) / length_squared);
        float _e43 = _group_1_binding_0_fs.params.z;
        vec2 _e44 = tile_gradient(t_6, _e43);
        vec4 _e46 = gradient_color(_e44.x, count_2);
        color_2 = (_e46 * _e44.y);
    } else {
        if (((kind_1 > 1.5) && (kind_1 < 2.5))) {
            vec2 _e54 = gradient_space(in_3);
            float _e59 = _group_1_binding_0_fs.params.z;
            vec2 _e60 = tile_gradient(length(_e54), _e59);
            vec4 _e62 = gradient_color(_e60.x, count_2);
            color_2 = (_e62 * _e60.y);
        } else {
            if (((kind_1 > 2.5) && (kind_1 < 3.5))) {
                vec2 _e70 = gradient_space(in_3);
                float start_angle = _group_1_binding_0_fs.geometry.z;
                float _e78 = _group_1_binding_0_fs.geometry.w;
                float sweep = max((_e78 - start_angle), 1e-6);
                float angle = (atan(-(_e70.y), -(_e70.x)) + (6.2831855 * 0.5));
                float _e96 = _group_1_binding_0_fs.params.z;
                vec2 _e97 = tile_gradient(((angle - start_angle) / sweep), _e96);
                vec4 _e99 = gradient_color(_e97.x, count_2);
                color_2 = (_e99 * _e97.y);
            } else {
                if (((kind_1 > 8.5) && (kind_1 < 9.5))) {
                    vec2 _e107 = gradient_space(in_3);
                    float separation = _group_1_binding_0_fs.params.w;
                    float r0_ = _group_1_binding_0_fs.geometry.z;
                    float dr = _group_1_binding_0_fs.geometry.w;
                    float a_2 = ((separation * separation) - (dr * dr));
                    float b_1 = ((_e107.x * separation) + (r0_ * dr));
                    float c_6 = (dot(_e107, _e107) - (r0_ * r0_));
                    float magnitude = max((separation * separation), (dr * dr));
                    if ((abs(a_2) > (magnitude * 1e-5))) {
                        float disc = ((b_1 * b_1) - (a_2 * c_6));
                        if ((disc >= 0.0)) {
                            float root = sqrt(disc);
                            float far = max(((b_1 + root) / a_2), ((b_1 - root) / a_2));
                            float near = min(((b_1 + root) / a_2), ((b_1 - root) / a_2));
                            if (((r0_ + (far * dr)) >= 0.0)) {
                                t_4 = far;
                                covered = true;
                            } else {
                                if (((r0_ + (near * dr)) >= 0.0)) {
                                    t_4 = near;
                                    covered = true;
                                }
                            }
                        }
                    } else {
                        if ((abs(b_1) > 1e-6)) {
                            float only = (c_6 / (2.0 * b_1));
                            if (((r0_ + (only * dr)) >= 0.0)) {
                                t_4 = only;
                                covered = true;
                            }
                        }
                    }
                    float _e178 = t_4;
                    float _e182 = _group_1_binding_0_fs.params.z;
                    vec2 _e183 = tile_gradient(_e178, _e182);
                    vec4 _e185 = gradient_color(_e183.x, count_2);
                    bool _e190 = covered;
                    color_2 = ((_e185 * _e183.y) * (_e190 ? 1.0 : 0.0));
                }
            }
        }
    }
    switch(int((kind_1 + 0.5))) {
        case 4: {
            vec4 _e197 = sample_image(in_3.clip);
            return _e197;
        }
        case 7: {
            vec3 _e198 = rounded_rect_space(in_3);
            vec4 _e199 = rounded_rect_coverage(_e198);
            return _e199;
        }
        case 8: {
            vec4 _e201 = ellipse_coverage(in_3.clip);
            return _e201;
        }
        case 12: {
            vec4 _e203 = rrect_blur_coverage(in_3.clip);
            return _e203;
        }
        case 13: {
            vec4 _e205 = point_field_coverage(in_3.uv);
            return _e205;
        }
        case 14: {
            vec4 _e207 = shadow_mesh_color(in_3.uv);
            return _e207;
        }
        case 6: {
            vec4 _e209 = blur_along_axis(in_3.clip);
            return _e209;
        }
        case 11: {
            vec4 _e211 = morphology_along_axis(in_3.clip);
            return _e211;
        }
        case 10: {
            vec4 _e213 = sample_mesh(in_3.uv);
            return _e213;
        }
        default: {
            break;
        }
    }
    if (((kind_1 > 4.5) && (kind_1 < 5.5))) {
        vec4 _e223 = textureLod(_group_0_binding_0_fs, vec2(in_3.uv), 0.0);
        float coverage_2 = _e223.x;
        vec4 tint_7 = _group_1_binding_0_fs.stops[0];
        float alpha_8 = (tint_7.w * coverage_2);
        return vec4((tint_7.xyz * alpha_8), alpha_8);
    }
    vec4 _e234 = color_2;
    float _e237 = color_2.w;
    float _e240 = color_2.w;
    return vec4((_e234.xyz * _e237), _e240);
}

void main() {
    VertexOutput in_ = VertexOutput(gl_FragCoord, _vs2fs_location0, _vs2fs_location1, _vs2fs_location2);
    vec4 tinted = vec4(0.0);
    vec4 _e1 = shade(in_);
    float _e5 = _group_1_binding_0_fs.filter_params.y;
    int tint_mode = int((_e5 + 0.5));
    if ((tint_mode == 13)) {
        tinted = (in_.tint * _e1);
    } else {
        vec4 _e15 = blend_tint(tint_mode, _e1, in_.tint);
        tinted = _e15;
    }
    vec4 _e16 = tinted;
    float _e20 = _group_1_binding_0_fs.filter_params.w;
    tinted = (_e16 * _e20);
    vec4 _e22 = tinted;
    vec4 _e23 = filtered(_e22);
    vec4 _e26 = dithered(_e23, in_.position.xy);
    _fs2p_location0 = _e26;
    return;
}

