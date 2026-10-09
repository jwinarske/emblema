// A caller's fragment program, standing in for one in the tests.
//
// This is not part of the renderer. It exists to prove the path a runtime
// effect actually takes: a fragment module this renderer's own shader knows
// nothing about, registered with a context, and turned into a pipeline that
// draws. It lives here because the build already translates WGSL to both
// backends' payloads, which is exactly what a caller's own build has to do --
// so the tests exercise the real arrangement rather than a special case.
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

// A vertical split at a threshold the caller sets, in the two colors the
// caller gives. Deliberately something no material here can draw, so a test
// showing it is a test that the caller's program ran and not that some
// built-in path produced a similar picture.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The varying through a `var` before it is read, which is not a style
    // choice. On a GC7000UL this entry point reading `in.clip` directly gets
    // the `position` builtin instead -- and so does every other varying it
    // names, `uv` and `tint` alike. A `var` is enough to fix it and a `let` is
    // not, because only the `var` becomes a real variable rather than another
    // name for the same extract. `docs/on-a-board.md` has the measurement.
    //
    // It belongs in this fixture rather than in the renderer because an effect
    // is somebody else's program: the renderer cannot write a caller's shader,
    // so the constraint is one a caller has to know about.
    var clip: vec3<f32> = in.clip;
    let threshold = paint.geometry.x;
    let color = select(paint.stops[1], paint.stops[0], clip.x / clip.z < threshold);
    return vec4<f32>(color.rgb * color.a, color.a);
}
