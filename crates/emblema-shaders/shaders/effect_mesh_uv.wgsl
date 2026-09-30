// A caller's fragment program that reads the mesh's own coordinate.
//
// The sibling of `effect.wgsl`: it reads `uv` where that one reads the clip
// position. That is the whole of what it
// exists to show. `draw_vertices` states a coordinate per vertex, and the
// question this answers is whether one reaches a caller's program -- which
// was written down as a thing that could not happen, on the reasoning that a
// program replaces this renderer's fragment shader and so has nowhere for the
// attribute to arrive. It has somewhere: the vertex stage is the renderer's
// own whatever the fragment does, and it hands on `uv` beside `clip`.
//
// The interface is the paint's uniform block, unchanged. An effect declares
// the same layout and reads the floats a caller packed into it, which is what
// lets one exist without a second descriptor set.
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

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) clip: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
};

// Present so the build emits a vertex stage for this source like any other.
// Nothing uses it: a pipeline running an effect takes its vertex stage from
// the renderer's own shader, because an effect replaces what a fragment does
// with a paint and not how geometry reaches clip space.
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

// A ramp along the coordinate the vertices state, between the two colors the
// caller gives. A ramp rather than a split, because the picture it makes is
// the evidence: four quads that each state their own share of one range draw a
// single continuous ramp across all of them, and four that could not read the
// coordinate would each draw the whole ramp over again, four times, with three
// seams.
//
// It takes no threshold from the paint, and that is not laziness. A mesh writes
// its own things into `geometry` -- among them the flag saying where a built-in
// shader should measure from -- so the slot `effect.wgsl` reads its threshold
// out of is not a caller's to use on this path. The colors are still the
// caller's; the range is fixed because the question here is where `uv` runs.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = mix(paint.stops[0], paint.stops[1], clamp(in.uv.x, 0.0, 1.0));
    return vec4<f32>(color.rgb * color.a, color.a);
}
