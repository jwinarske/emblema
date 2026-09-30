// A caller's program that samples a texture, standing in for one in the tests.
//
// The point it exists to prove is that an effect needs no descriptor set of
// its own to read an image. Every draw already binds a texture at the one
// binding this renderer's shader declares -- a placeholder where the material
// samples nothing -- so a program declaring the same binding gets whatever the
// draw named, and the machinery that carries it is the machinery that was
// already there.
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

// The texture read across clip space and multiplied by a color the caller
// gives. Deliberately mapped from the fragment's own position rather than from
// a vertex coordinate, so the picture depends on the sampler having been bound
// and on nothing this renderer would have done for an image material.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = in.clip.xy / in.clip.z * 0.5 + vec2<f32>(0.5);
    let texel = textureSampleLevel(image_texture, image_sampler, coord, 0.0);
    let tint = paint.stops[0];
    let color = texel * tint;
    return vec4<f32>(color.rgb * color.a, color.a);
}
