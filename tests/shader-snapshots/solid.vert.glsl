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
layout(location = 0) in vec3 _p2vs_location0;
layout(location = 1) in vec2 _p2vs_location1;
layout(location = 2) in vec4 _p2vs_location2;
smooth out vec3 _vs2fs_location0;
smooth out vec2 _vs2fs_location1;
smooth out vec4 _vs2fs_location2;

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

float cubic_weight(float x) {
    float t_4 = abs(x);
    if ((t_4 < 1.0)) {
        float cubic_1 = ((12.0 - (9.0 * 0.33333334)) - (6.0 * 0.33333334));
        float square = ((-18.0 + (12.0 * 0.33333334)) + (6.0 * 0.33333334));
        return ((((((cubic_1 * t_4) + square) * t_4) * t_4) + (6.0 - (2.0 * 0.33333334))) / 6.0);
    }
    if ((t_4 < 2.0)) {
        float cubic_2 = (-(0.33333334) - (6.0 * 0.33333334));
        float square_1 = ((6.0 * 0.33333334) + (30.0 * 0.33333334));
        float linear = ((-12.0 * 0.33333334) - (48.0 * 0.33333334));
        return (((((((cubic_2 * t_4) + square_1) * t_4) + linear) * t_4) + ((8.0 * 0.33333334) + (24.0 * 0.33333334))) / 6.0);
    }
    return 0.0;
}

vec2 tile_uv(vec2 uv_3, float tile_1) {
    if (((tile_1 > 0.5) && (tile_1 < 1.5))) {
        return fract(uv_3);
    }
    if ((tile_1 > 2.5)) {
        return (vec2(1.0) - abs((vec2(1.0) - (uv_3 - (2.0 * floor((uv_3 * 0.5)))))));
    }
    return clamp(uv_3, vec2(0.0), vec2(1.0));
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

vec3 linear_to_srgb(vec3 c) {
    vec3 m = abs(c);
    vec3 low_2 = (m * 12.92);
    vec3 high_2 = ((1.055 * pow(m, vec3(0.41666666))) - vec3(0.055));
    return (sign(c) * mix(high_2, low_2, lessThanEqual(m, vec3(0.0031308))));
}

vec3 srgb_to_linear(vec3 c_1) {
    vec3 m_1 = abs(c_1);
    vec3 low_3 = (m_1 / vec3(12.92));
    vec3 high_3 = pow(((m_1 + vec3(0.055)) / vec3(1.055)), vec3(2.4));
    return (sign(c_1) * mix(high_3, low_3, lessThanEqual(m_1, vec3(0.04045))));
}

float sat(vec3 c_2) {
    return (max(c_2.x, max(c_2.y, c_2.z)) - min(c_2.x, min(c_2.y, c_2.z)));
}

vec3 set_sat(vec3 c_3, float s) {
    float low_4 = min(c_3.x, min(c_3.y, c_3.z));
    float high_4 = max(c_3.x, max(c_3.y, c_3.z));
    if ((high_4 <= low_4)) {
        return vec3(0.0);
    }
    return (((c_3 - vec3(low_4)) * s) / vec3((high_4 - low_4)));
}

float lum(vec3 c_4) {
    return dot(c_4, vec3(0.3, 0.59, 0.11));
}

vec3 set_lum(vec3 c_5, float l) {
    vec3 out_1 = vec3(0.0);
    float _e2 = lum(c_5);
    vec3 shifted = (c_5 + vec3((l - _e2)));
    float low_5 = min(shifted.x, min(shifted.y, shifted.z));
    float high_5 = max(shifted.x, max(shifted.y, shifted.z));
    float _e16 = lum(shifted);
    out_1 = shifted;
    if ((low_5 < 0.0)) {
        vec3 _e20 = out_1;
        out_1 = (vec3(_e16) + (((_e20 - vec3(_e16)) * _e16) / vec3(max((_e16 - low_5), 1e-8))));
    }
    if ((high_5 > 1.0)) {
        vec3 _e33 = out_1;
        out_1 = (vec3(_e16) + (((_e33 - vec3(_e16)) * (1.0 - _e16)) / vec3(max((high_5 - _e16), 1e-8))));
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

float ordered_dither(vec2 frag) {
    uint x_2 = (uint(frag.x) % 8u);
    uint y = (uint(frag.y) ^ x_2);
    uint m_2 = (((((((y & 1u) << 5u) | ((x_2 & 1u) << 4u)) | ((y & 2u) << 2u)) | ((x_2 & 2u) << 1u)) | ((y & 4u) >> 1u)) | ((x_2 & 4u) >> 2u));
    return ((float(m_2) * 0.015625) - 0.4921875);
}

void main() {
    vec3 position = _p2vs_location0;
    vec2 uv = _p2vs_location1;
    vec4 tint = _p2vs_location2;
    VertexOutput out_ = VertexOutput(vec4(0.0), vec3(0.0), vec2(0.0), vec4(0.0));
    out_.position = vec4(position.xy, 0.0, position.z);
    out_.clip = position;
    out_.uv = uv;
    out_.tint = tint;
    VertexOutput _e12 = out_;
    gl_Position = _e12.position;
    _vs2fs_location0 = _e12.clip;
    _vs2fs_location1 = _e12.uv;
    _vs2fs_location2 = _e12.tint;
    gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);
    return;
}

