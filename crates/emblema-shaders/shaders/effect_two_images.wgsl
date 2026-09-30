// A caller's program that samples two textures, standing in for one in the tests.
//
// One texture was the case the shared descriptor set already covered, since
// every draw binds one at the binding this renderer's own shader declares. Two
// is the case that needed the layout to grow: the extra images are further
// bindings in the same set rather than a set of their own, which is what keeps
// one layout serving every pipeline.
//
// Deliberately combines them with an operation neither texture could produce
// alone -- the difference of the two -- so a picture drawn with the same
// texture bound twice, or with the second binding left at the placeholder, is
// not the picture this makes.
struct Paint {
    stops: array<vec4<f32>, 4>,
    offsets: vec4<f32>,
    geometry: vec4<f32>,
    to_local: array<vec4<f32>, 3>,
    params: vec4<f32>,
    recolor: array<vec4<f32>, 4>,
    filter_offset: vec4<f32>,
    filter_params: vec4<f32>,
};

@group(1) @binding(0) var<uniform> paint: Paint;
@group(0) @binding(0) var image_texture: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
// Binding two rather than one: the sampler holds one, and every image after
// the first counts from two. Both backends number them that way.
@group(0) @binding(2) var second_texture: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position.xy, 0.0, position.z);
    out.clip = position;
    out.uv = uv;
    out.tint = tint;
    return out;
}

// Both textures read across clip space, and their difference tinted. The
// difference rather than a sum or a blend because it is the operation that
// cannot be mistaken for either texture alone: where the two agree it is black
// whatever they hold, and where they differ it is exactly how much.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = in.clip.xy / in.clip.z * 0.5 + vec2<f32>(0.5);
    let first = textureSampleLevel(image_texture, image_sampler, coord, 0.0);
    let second = textureSampleLevel(second_texture, image_sampler, coord, 0.0);
    let tint = paint.stops[0];
    let difference = abs(first.rgb - second.rgb);
    let alpha = tint.a;
    return vec4<f32>(difference * tint.rgb * alpha, alpha);
}
