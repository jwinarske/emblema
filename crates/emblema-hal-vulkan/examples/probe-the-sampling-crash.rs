//! What a driver needs before sampling a texture takes it down.
//!
//! An i.MX8MP's Vivante GC7000UL segfaults inside `vkCmdDrawIndexed` on any
//! draw that samples a texture. `docs/on-a-board.md` has the census that
//! localized it: ten of twelve suites pass, `sampling` and `runtime_effect`
//! do not, and the one test in `sampling` that samples *nothing* is the one
//! that passes.
//!
//! That is already a one-draw reproduction, but it is a reproduction through
//! this renderer. This is the same question asked with nothing but `ash`, so
//! that "our descriptor bookkeeping is contributing" stops being one of the
//! answers -- the same thing the composite-construct probe beside this one
//! did for pipeline creation.
//!
//! The variants go up in one step each:
//!
//! | variant | pipeline | descriptor set holds | records |
//! |---|---|---|---|
//! | `dispatch-plain` | compute | a storage buffer | a dispatch |
//! | `dispatch-sampled` | compute | that, and a sampled image | a dispatch |
//! | `dispatch-sampled-unused` | compute | that, and a sampled image the shader never reads | a dispatch |
//!
//! Compute first because it is a tenth of the setup of a draw and because the
//! answer decides what to build next. If a dispatch that samples crashes, the
//! reproduction is this file. If it does not, the bug wants a graphics
//! pipeline and the next probe has to build one.
//!
//! `dispatch-sampled-unused` is the control that separates *binding* an image
//! from *sampling* one: same descriptor set, same pipeline layout, a shader
//! that ignores it.
//!
//! One variant per invocation, since a crash takes the process with it. With
//! no argument it lists them.

// An example is a program. It reports by exiting.
#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use ash::vk;

const VARIANTS: &[(&str, &str, bool)] = &[
    (
        "dispatch-plain",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;

@compute @workgroup_size(1)
fn cs_main() {
    out[0] = out[1] * 2.0;
}
"#,
        false,
    ),
    (
        "dispatch-sampled",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@compute @workgroup_size(1)
fn cs_main() {
    let c = textureSampleLevel(tex, samp, vec2<f32>(0.5, 0.5), 0.0);
    out[0] = c.r + c.g + c.b + c.a;
}
"#,
        true,
    ),
    (
        "dispatch-sampled-unused",
        r#"
@group(0) @binding(0) var<storage, read_write> out: array<f32>;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@compute @workgroup_size(1)
fn cs_main() {
    // `tex` and `samp` are declared and bound and never read, which is what
    // separates holding a sampled image in the set from sampling it.
    out[0] = out[1] * 2.0;
}
"#,
        true,
    ),
];

/// The graphics trio, which is where the crash actually lives.
///
/// Same three-way split as the compute set: no image, an image bound and not
/// read, an image sampled. The vertex shader builds a triangle from
/// `vertex_index` so there is no vertex buffer to get wrong, and the draw is
/// `vkCmdDrawIndexed` because that is the call in the backtrace.
const GRAPHICS: &[(&str, &str, bool)] = &[
    (
        "draw-plain",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.25, 0.5, 0.75, 1.0);
}
"#,
        false,
    ),
    (
        "draw-sampled-unused",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Bound, never read.
    return vec4<f32>(0.25, 0.5, 0.75, 1.0);
}
"#,
        true,
    ),
    (
        "draw-sampled",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, vec2<f32>(0.5, 0.5), 0.0);
}
"#,
        true,
    ),
    // The renderer's own shape. Its layout declares four sampled-image
    // bindings whatever the shader uses, and the path that binds a real
    // texture writes only as many as the caller supplied -- so three of the
    // four are left unwritten. The placeholder path writes all four, and the
    // one renderer test that passes is the one that takes it.
    //
    // `draw-full-set` is the same layout with every slot written. If that
    // passes and `draw-sparse-set` crashes, it is the unwritten descriptors
    // and not the sampling.
    (
        "draw-sparse-set",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, vec2<f32>(0.5, 0.5), 0.0);
}
"#,
        true,
    ),
    (
        "draw-full-set",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, vec2<f32>(0.5, 0.5), 0.0);
}
"#,
        true,
    ),
    // `solid.wgsl` never samples in its entry point: `fs_main` calls `shade`,
    // which calls `sample_image` and the rest, and those do the sampling. The
    // probe above samples in `fs_main` directly, which is the one structural
    // difference between a twenty-line shader that passes and a
    // fifteen-hundred-line one that does not -- and a called function was the
    // whole of the other defect on this driver.
    (
        "draw-sampled-in-callee",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn sampled() -> vec4<f32> {
    return textureSampleLevel(tex, samp, vec2<f32>(0.5, 0.5), 0.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return sampled();
}
"#,
        true,
    ),
    // Bisecting `solid.wgsl` put the crash in `shade`'s glyph arm, whose only
    // sample is `textureSampleLevel(tex, samp, in.uv, 0.0)` -- a coordinate
    // extracted from `shade`'s own value parameter. Every variant above
    // samples at a constant. This is that one difference.
    (
        "draw-sampled-param-coord",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }
struct In { uv: vec2<f32>, k: f32 }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn shade(i: In) -> vec4<f32> {
    return textureSampleLevel(tex, samp, i.uv, 0.0);
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    var i: In;
    i.uv = v.pos.xy * 0.001;
    i.k = 1.0;
    return shade(i);
}
"#,
        true,
    ),
    // Carving `shade` down to its preamble plus the glyph arm -- twenty-eight
    // lines -- still crashes, so it was never an interaction. What that arm
    // has and every variant above lacks is that the sample sits inside a
    // conditional, on a value the caller passed in.
    (
        "draw-sampled-in-branch",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }
struct In { uv: vec2<f32>, k: f32 }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn shade(i: In) -> vec4<f32> {
    if (i.k > 4.5 && i.k < 5.5) {
        let coverage = textureSampleLevel(tex, samp, i.uv, 0.0).r;
        return vec4<f32>(coverage, coverage, coverage, coverage);
    }
    return vec4<f32>(0.25, 0.5, 0.75, 1.0);
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    var i: In;
    i.uv = v.pos.xy * 0.001;
    i.k = 5.0;
    return shade(i);
}
"#,
        true,
    ),
    // The renderer's shape, and the one structural thing every variant above
    // lacks: `solid.wgsl` puts its texture and sampler in set zero and its
    // paint uniform in **set one**, so a draw binds two descriptor sets. This
    // does the same.
    (
        "draw-two-sets",
        r#"
struct VsOut { @builtin(position) pos: vec4<f32> }
struct Paint { stops: array<vec4<f32>, 4>, params: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var<uniform> paint: Paint;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32(i32(i) - 1);
    let y = f32(i32(i & 1u) * 2 - 1);
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

fn shade(uv: vec2<f32>) -> vec4<f32> {
    let kind = paint.params.y;
    if (kind > 4.5 && kind < 5.5) {
        let coverage = textureSampleLevel(tex, samp, uv, 0.0).r;
        let tint = paint.stops[0];
        let alpha = tint.a * coverage;
        return vec4<f32>(tint.rgb * alpha, alpha);
    }
    return paint.stops[0];
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return shade(v.pos.xy * 0.001);
}
"#,
        true,
    ),
];

/// The two-set shader, fed from a real vertex buffer.
///
/// Every variant above builds its position from `vertex_index` and declares no
/// vertex input at all. `solid.wgsl` takes three attributes -- a `vec3`
/// position, a `vec2` uv and a `vec4` tint -- out of a bound buffer, and the
/// uv it samples at is the interpolated one. Measured on an i.MX8MP, the
/// renderer's sampling draw appends 132 records to a 128-entry driver table
/// where its solid draw appends one; the variants above append none, so this
/// is the difference left to account for the path being entered at all.
const WITH_ATTRIBUTES: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}
struct Paint { stops: array<vec4<f32>, 4>, params: vec4<f32> }

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(1) @binding(0) var<uniform> paint: Paint;

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(position.xy, 0.0, position.z);
    out.uv = uv;
    out.tint = tint;
    return out;
}

fn shade(in: VsOut) -> vec4<f32> {
    let kind = paint.params.y;
    if (kind > 4.5 && kind < 5.5) {
        let coverage = textureSampleLevel(tex, samp, in.uv, 0.0).r;
        let tint = paint.stops[0];
        let alpha = tint.a * coverage;
        return vec4<f32>(tint.rgb * alpha, alpha);
    }
    return paint.stops[0];
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return v.tint * shade(v);
}
"#;

/// The two-set shader with forty functions nobody calls bolted on.
///
/// Carving `shade` to twenty-eight lines left a module of **2,495 words and
/// forty-two functions** -- naga keeps a function that nothing calls, so the
/// thirty-nine stubs stayed. Every variant in the tables above is two or
/// three functions and a few hundred words, which is the one difference left
/// between a probe that passes and a renderer that does not.
fn many_functions() -> String {
    let mut source = String::from(
        GRAPHICS
            .iter()
            .find(|(n, _, _)| *n == "draw-two-sets")
            .expect("the two-set variant")
            .1,
    );
    for i in 0..40 {
        source.push_str(&format!(
            "\nfn unused_{i}(x: vec4<f32>) -> vec4<f32> {{ return x * {i}.0; }}\n"
        ));
    }
    source
}

fn spirv(source: &str) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("parsing:\n{}", e.emit_to_string(source)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("validating: {e}"));
    naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
        .expect("translating")
}

/// The graphics half: a 64x64 pass, one triangle, `vkCmdDrawIndexed`.
///
/// No vertex buffer -- the triangle comes from `vertex_index` -- so the only
/// things bound are the pipeline, the index buffer and, for two of the three,
/// a descriptor set holding a sampled image.
// Every call below is a Vulkan one and the whole function is the unsafe
// region; marking each individually would be noise over signal.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn run_graphics(name: &str, words: &[u32], with_image: bool) -> std::process::ExitCode {
    const SIDE: u32 = 64;
    let entry = ash::Entry::load().expect("loading Vulkan");
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let instance = entry
        .create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )
        .expect("create_instance");
    let physical = *instance
        .enumerate_physical_devices()
        .expect("enumerate")
        .first()
        .expect("a Vulkan device");
    let props = instance.get_physical_device_properties(physical);
    println!(
        "device: {}",
        std::ffi::CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy()
    );
    let family = instance
        .get_physical_device_queue_family_properties(physical)
        .iter()
        .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .expect("a graphics queue") as u32;
    let priorities = [1.0f32];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    // `VulkanContext` asks for every extension in this list that the device
    // offers, and a bare device asks for none. Set `EMBLEMA_EXTENSIONS` to ask
    // for them here too.
    //
    // Measured on an i.MX8MP: six of the eight are offered, and enabling all
    // six leaves the append count at zero for every variant. So the extension
    // set is not what opens the driver's append path -- which the renderer
    // enters even for a solid draw and nothing here enters at all.
    let wanted = [
        c"VK_EXT_external_memory_dma_buf",
        c"VK_KHR_external_memory_fd",
        c"VK_EXT_image_drm_format_modifier",
        c"VK_KHR_external_fence_fd",
        c"VK_KHR_external_semaphore_fd",
        c"VK_EXT_physical_device_drm",
        c"VK_EXT_blend_operation_advanced",
        c"VK_KHR_swapchain",
    ];
    let available: Vec<std::ffi::CString> = instance
        .enumerate_device_extension_properties(physical)
        .expect("enumerate_device_extension_properties")
        .iter()
        .map(|p| std::ffi::CStr::from_ptr(p.extension_name.as_ptr()).to_owned())
        .collect();
    let enabled: Vec<*const std::ffi::c_char> = if std::env::var("EMBLEMA_EXTENSIONS").is_ok() {
        let names: Vec<_> = wanted
            .iter()
            .filter(|w| available.iter().any(|a| a.as_c_str() == **w))
            .collect();
        println!("enabling {} of the renderer's extensions", names.len());
        names.into_iter().map(|w| w.as_ptr()).collect()
    } else {
        Vec::new()
    };
    let device = instance
        .create_device(
            physical,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&enabled),
            None,
        )
        .expect("create_device");
    let queue = device.get_device_queue(family, 0);
    let memory_props = instance.get_physical_device_memory_properties(physical);
    let pick = |bits: u32, want: vk::MemoryPropertyFlags| -> u32 {
        (0..memory_props.memory_type_count)
            .find(|i| {
                bits & (1 << i) != 0
                    && memory_props.memory_types[*i as usize]
                        .property_flags
                        .contains(want)
            })
            .expect("a memory type")
    };
    let make_image = |usage: vk::ImageUsageFlags, side: u32| {
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D {
                        width: side,
                        height: side,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .expect("create_image");
        let need = device.get_image_memory_requirements(image);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(pick(
                        need.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )),
                None,
            )
            .expect("allocate image memory");
        device
            .bind_image_memory(image, memory, 0)
            .expect("bind_image_memory");
        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            // `EMBLEMA_REMAINING` asks for the sentinel the
                            // renderer used to ask for. The image has one
                            // level, so the two denote the same single level.
                            //
                            // `sampled` puts it only on the image a shader
                            // reads, which is where the renderer had it;
                            // `all` puts it on the color target's view too.
                            // The distinction matters: a framebuffer
                            // attachment is a different code path, and the
                            // whole point is to reproduce the renderer's.
                            .level_count(match std::env::var("EMBLEMA_REMAINING").as_deref() {
                                Ok("all") => vk::REMAINING_MIP_LEVELS,
                                Ok("sampled") if usage.contains(vk::ImageUsageFlags::SAMPLED) => {
                                    vk::REMAINING_MIP_LEVELS
                                }
                                _ => 1,
                            })
                            .layer_count(1),
                    ),
                None,
            )
            .expect("create_image_view");
        (image, memory, view)
    };

    let (target, target_memory, target_view) =
        make_image(vk::ImageUsageFlags::COLOR_ATTACHMENT, SIDE);
    let mut sampled = (
        vk::Image::null(),
        vk::DeviceMemory::null(),
        vk::ImageView::null(),
    );
    let mut sampler = vk::Sampler::null();
    if with_image {
        // The renderer's textures are `TextureDescriptor::offscreen`, whose
        // usage is sampled *and* color attachment and transfer. A probe that
        // asks only for `SAMPLED` is not the same image, and
        // `EMBLEMA_SAMPLED_USAGE=offscreen` closes that gap.
        let sampled_usage = if std::env::var("EMBLEMA_SAMPLED_USAGE").as_deref() == Ok("offscreen")
        {
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
        } else {
            vk::ImageUsageFlags::SAMPLED
        };
        sampled = make_image(sampled_usage, 2);
        sampler = device
            .create_sampler(&vk::SamplerCreateInfo::default(), None)
            .expect("create_sampler");
    }

    let attachment = [vk::AttachmentDescription::default()
        .format(vk::Format::R8G8B8A8_UNORM)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::GENERAL)];
    let color_ref = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref)];
    let render_pass = device
        .create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachment)
                .subpasses(&subpass),
            None,
        )
        .expect("create_render_pass");
    let views = [target_view];
    let framebuffer = device
        .create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&views)
                .width(SIDE)
                .height(SIDE)
                .layers(1),
            None,
        )
        .expect("create_framebuffer");

    // How many sampled-image bindings the layout declares, and how many get
    // written. `draw-sparse-set` mirrors the renderer: four declared, one
    // written, three left undefined.
    let (declared, written) = match name {
        "draw-sparse-set" => (4usize, 1usize),
        "draw-full-set" => (4, 4),
        _ => (1, 1),
    };
    // Whether the paint uniform gets a descriptor set of its own, as
    // `solid.wgsl` gives it.
    let two_sets = name == "draw-two-sets"
        || name == "draw-dynamic-offset"
        || name == "draw-vertex-attributes";
    // `record_draw` binds the material set as `UNIFORM_BUFFER_DYNAMIC` at
    // `firstSet` one with a dynamic offset per draw, which is the line
    // immediately above the `cmd_draw_indexed` the driver dies inside.
    let dynamic = name == "draw-dynamic-offset" || name == "draw-vertex-attributes";
    // Three attributes out of a bound buffer, as `vs_main` declares them.
    let attributes = name == "draw-vertex-attributes";
    let paint_type = if dynamic {
        vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC
    } else {
        vk::DescriptorType::UNIFORM_BUFFER
    };
    let mut bindings = Vec::new();
    if with_image {
        for extra in 1..declared {
            // Binding numbers two upward, as the renderer lays them out.
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(extra as u32 + 1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
        }
        // The first image is binding zero and the sampler is binding one,
        // which is the numbering the shaders above use and the one the
        // renderer uses. Getting these out of step with the shader is invalid
        // usage, and this driver answers invalid usage by corrupting its heap
        // -- which looks like a find and is not one.
        bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        );
        bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        );
    }
    let set_layout = device
        .create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )
        .expect("create_descriptor_set_layout");
    let uniform_binding = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(paint_type)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let paint_layout = if two_sets {
        device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&uniform_binding),
                None,
            )
            .expect("create_descriptor_set_layout (paint)")
    } else {
        vk::DescriptorSetLayout::null()
    };
    let set_layouts: Vec<vk::DescriptorSetLayout> = if two_sets {
        vec![set_layout, paint_layout]
    } else {
        vec![set_layout]
    };
    let layout = device
        .create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
            None,
        )
        .expect("create_pipeline_layout");

    let mut set = vk::DescriptorSet::null();
    let mut pool = vk::DescriptorPool::null();
    if with_image {
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(declared as u32),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(1),
        ];
        let sizes: Vec<vk::DescriptorPoolSize> = if two_sets {
            sizes
                .into_iter()
                .chain([vk::DescriptorPoolSize::default()
                    .ty(paint_type)
                    .descriptor_count(1)])
                .collect()
        } else {
            sizes.to_vec()
        };
        pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(if two_sets { 2 } else { 1 })
                    .pool_sizes(&sizes),
                None,
            )
            .expect("create_descriptor_pool");
        // The image set alone; the paint set is allocated below from its own
        // layout, because they are different sets and not one array.
        let image_layouts = [set_layout];
        set = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&image_layouts),
            )
            .expect("allocate_descriptor_sets")[0];
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(sampled.2)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        // Images at binding zero and two upward, the sampler at one, which is
        // the renderer's own numbering. `written` of the `declared` image
        // slots get a view; the rest are left undefined on purpose.
        let mut writes = vec![vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image_info)];
        for extra in 1..written {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(extra as u32 + 1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_info),
            );
        }
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
        );
        println!("descriptor set: {declared} image binding(s) declared, {written} written");
        device.update_descriptor_sets(&writes, &[]);
    }

    // The paint uniform, in a set of its own.
    let mut paint_set = vk::DescriptorSet::null();
    let mut paint_offset = 0u32;
    let mut paint_buffer = vk::Buffer::null();
    let mut paint_memory = vk::DeviceMemory::null();
    if two_sets {
        // Five vec4s: four stops and the params, which is what the shader
        // declares. A dynamic binding gets two of them, padded to the device's
        // minimum offset alignment, so that the offset bound below is not zero
        // -- `materials.rs` pads the same way and for the same reason.
        let material = 5 * 16u64;
        let align = instance
            .get_physical_device_properties(physical)
            .limits
            .min_uniform_buffer_offset_alignment;
        let stride = material.next_multiple_of(align);
        let size = if dynamic { 2 * stride } else { material };
        paint_buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                None,
            )
            .expect("create_buffer (paint)");
        let need = device.get_buffer_memory_requirements(paint_buffer);
        paint_memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(pick(
                        need.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .expect("allocate paint memory");
        device
            .bind_buffer_memory(paint_buffer, paint_memory, 0)
            .expect("bind_buffer_memory (paint)");
        // `params.y` is the material kind, and five is the one whose arm
        // samples. Writing it means the sampling branch is the live one.
        let mapped = device
            .map_memory(paint_memory, 0, need.size, vk::MemoryMapFlags::empty())
            .expect("map_memory (paint)") as *mut f32;
        std::ptr::write_bytes(mapped, 0, (need.size / 4) as usize);
        // `params.y` sits sixty-eight bytes into a material, and the live one
        // is the second when a dynamic offset selects it.
        let base = if dynamic { stride } else { 0 };
        paint_offset = u32::try_from(base).expect("the offset fits");
        mapped.byte_add((base + 68) as usize).write(5.0);
        device.unmap_memory(paint_memory);

        let paint_layouts = [paint_layout];
        paint_set = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&paint_layouts),
            )
            .expect("allocate_descriptor_sets (paint)")[0];
        // The range is one material and not the whole buffer, which is what
        // `materials.rs` names: the dynamic offset is added to it.
        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(paint_buffer)
            .range(material)];
        device.update_descriptor_sets(
            &[vk::WriteDescriptorSet::default()
                .dst_set(paint_set)
                .dst_binding(0)
                .descriptor_type(paint_type)
                .buffer_info(&buffer_info)],
            &[],
        );
        println!("paint uniform in a second descriptor set");
    }

    let module = device
        .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
        .expect("create_shader_module");
    let vs = std::ffi::CString::new("vs_main").unwrap();
    let fs = std::ffi::CString::new("fs_main").unwrap();
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(module)
            .name(&vs),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(module)
            .name(&fs),
    ];
    // Nine floats a vertex: the position, the uv and the tint, in the order
    // `vs_main` declares them and at the offsets the renderer packs them to.
    let vertex_bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(36)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let vertex_attributes = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(12),
        vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R32G32B32A32_SFLOAT)
            .offset(20),
    ];
    let vertex_input = if attributes {
        vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&vertex_bindings)
            .vertex_attribute_descriptions(&vertex_attributes)
    } else {
        vk::PipelineVertexInputStateCreateInfo::default()
    };
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewports = [vk::Viewport::default()
        .width(SIDE as f32)
        .height(SIDE as f32)
        .max_depth(1.0)];
    let scissors = [vk::Rect2D::default().extent(vk::Extent2D {
        width: SIDE,
        height: SIDE,
    })];
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewports(&viewports)
        .scissors(&scissors);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
    println!("creating the pipeline");
    let pipelines = device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &[vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
                .color_blend_state(&blend)
                .layout(layout)
                .render_pass(render_pass)
                .subpass(0)],
            None,
        )
        .expect("create_graphics_pipelines");
    println!("pipeline created");

    // Three indices, so the draw below is the indexed one the backtrace names;
    // a quad's six where there is a vertex buffer to read them out of, which is
    // what the renderer's batch pushes.
    let indices: &[u32] = if attributes {
        &[0, 1, 2, 0, 2, 3]
    } else {
        &[0, 1, 2]
    };
    let index_buffer = device
        .create_buffer(
            &vk::BufferCreateInfo::default()
                .size(std::mem::size_of_val(indices) as u64)
                .usage(vk::BufferUsageFlags::INDEX_BUFFER),
            None,
        )
        .expect("create_buffer");
    let need = device.get_buffer_memory_requirements(index_buffer);
    let index_memory = device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(need.size)
                .memory_type_index(pick(
                    need.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )),
            None,
        )
        .expect("allocate index memory");
    device
        .bind_buffer_memory(index_buffer, index_memory, 0)
        .expect("bind_buffer_memory");
    let mapped = device
        .map_memory(index_memory, 0, need.size, vk::MemoryMapFlags::empty())
        .expect("map_memory") as *mut u32;
    std::ptr::copy_nonoverlapping(indices.as_ptr(), mapped, indices.len());
    device.unmap_memory(index_memory);

    // The four corners of the target, each with a uv and an opaque white tint.
    let mut vertex_buffer = vk::Buffer::null();
    let mut vertex_memory = vk::DeviceMemory::null();
    if attributes {
        #[rustfmt::skip]
        let vertices: [f32; 36] = [
            -1.0, -1.0, 1.0,  0.0, 0.0,  1.0, 1.0, 1.0, 1.0,
             1.0, -1.0, 1.0,  1.0, 0.0,  1.0, 1.0, 1.0, 1.0,
             1.0,  1.0, 1.0,  1.0, 1.0,  1.0, 1.0, 1.0, 1.0,
            -1.0,  1.0, 1.0,  0.0, 1.0,  1.0, 1.0, 1.0, 1.0,
        ];
        vertex_buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(std::mem::size_of_val(&vertices) as u64)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER),
                None,
            )
            .expect("create_buffer (vertices)");
        let need = device.get_buffer_memory_requirements(vertex_buffer);
        vertex_memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(pick(
                        need.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .expect("allocate vertex memory");
        device
            .bind_buffer_memory(vertex_buffer, vertex_memory, 0)
            .expect("bind_buffer_memory (vertices)");
        let mapped = device
            .map_memory(vertex_memory, 0, need.size, vk::MemoryMapFlags::empty())
            .expect("map_memory (vertices)") as *mut f32;
        std::ptr::copy_nonoverlapping(vertices.as_ptr(), mapped, vertices.len());
        device.unmap_memory(vertex_memory);
    }

    let command_pool = device
        .create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(family),
            None,
        )
        .expect("create_command_pool");
    let cmd = device
        .allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
        .expect("allocate_command_buffers")[0];
    device
        .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
        .expect("begin");
    if with_image {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .image(sampled.0)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                )],
        );
    }
    let clear = [vk::ClearValue {
        color: vk::ClearColorValue {
            float32: [0.0, 0.0, 0.0, 1.0],
        },
    }];
    device.cmd_begin_render_pass(
        cmd,
        &vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D::default().extent(vk::Extent2D {
                width: SIDE,
                height: SIDE,
            }))
            .clear_values(&clear),
        vk::SubpassContents::INLINE,
    );
    device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipelines[0]);
    if with_image {
        let sets: Vec<vk::DescriptorSet> = if two_sets && !dynamic {
            vec![set, paint_set]
        } else {
            vec![set]
        };
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            0,
            &sets,
            &[],
        );
        if dynamic {
            // Set one on its own, with the offset traveling in the binding
            // call -- the shape `record_draw` uses per draw.
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                1,
                &[paint_set],
                &[paint_offset],
            );
        }
    }
    if attributes {
        device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
    }
    device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
    println!("recording the draw");
    device.cmd_draw_indexed(cmd, indices.len() as u32, 1, 0, 0, 0);
    println!("draw recorded");
    device.cmd_end_render_pass(cmd);
    device.end_command_buffer(cmd).expect("end");

    println!("submitting");
    let buffers = [cmd];
    device
        .queue_submit(
            queue,
            &[vk::SubmitInfo::default().command_buffers(&buffers)],
            vk::Fence::null(),
        )
        .expect("submit");
    device.queue_wait_idle(queue).expect("wait");
    println!("completed");
    let _ = name;

    device.destroy_command_pool(command_pool, None);
    device.destroy_buffer(index_buffer, None);
    device.free_memory(index_memory, None);
    if attributes {
        device.destroy_buffer(vertex_buffer, None);
        device.free_memory(vertex_memory, None);
    }
    device.destroy_pipeline(pipelines[0], None);
    device.destroy_shader_module(module, None);
    if with_image {
        device.destroy_descriptor_pool(pool, None);
        device.destroy_sampler(sampler, None);
        device.destroy_image_view(sampled.2, None);
        device.destroy_image(sampled.0, None);
        device.free_memory(sampled.1, None);
    }
    if two_sets {
        device.destroy_buffer(paint_buffer, None);
        device.free_memory(paint_memory, None);
        device.destroy_descriptor_set_layout(paint_layout, None);
    }
    device.destroy_pipeline_layout(layout, None);
    device.destroy_descriptor_set_layout(set_layout, None);
    device.destroy_framebuffer(framebuffer, None);
    device.destroy_render_pass(render_pass, None);
    device.destroy_image_view(target_view, None);
    device.destroy_image(target, None);
    device.free_memory(target_memory, None);
    device.destroy_device(None);
    instance.destroy_instance(None);
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    let Some(wanted) = std::env::args().nth(1) else {
        println!("variants:");
        println!(
            "  {:26} the two-set shader, bound the way `record_draw` binds it",
            "draw-dynamic-offset"
        );
        println!(
            "  {:26} that, fed from a real vertex buffer",
            "draw-vertex-attributes"
        );
        println!(
            "  {:26} the two-set shader plus forty uncalled functions",
            "draw-many-functions"
        );
        for (name, _, samples) in VARIANTS.iter().chain(GRAPHICS) {
            println!(
                "  {name:26} {}",
                if *samples {
                    "binds a sampled image"
                } else {
                    "storage buffer only"
                }
            );
        }
        return std::process::ExitCode::SUCCESS;
    };
    // `--dump <dir>` writes each graphics variant's module out, so a C
    // harness can be handed the same bytes. `probe-the-composite-rule` carries
    // the same switch for the same reason.
    if wanted == "--dump" {
        let dir = std::env::args().nth(2).expect("a directory to dump into");
        std::fs::create_dir_all(&dir).expect("the dump directory");
        for (name, source, _) in GRAPHICS {
            let words = spirv(source);
            let mut bytes = Vec::with_capacity(words.len() * 4);
            for word in &words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            let path = format!("{dir}/{name}.spv");
            std::fs::write(&path, &bytes).expect("writing the module");
            println!("{path}: {} words", words.len());
        }
        return std::process::ExitCode::SUCCESS;
    }
    if wanted == "draw-vertex-attributes" {
        let words = spirv(WITH_ATTRIBUTES);
        println!("draw-vertex-attributes: {} words, graphics", words.len());
        // SAFETY: as below.
        return unsafe { run_graphics("draw-vertex-attributes", &words, true) };
    }
    if wanted == "draw-dynamic-offset" {
        let source = GRAPHICS
            .iter()
            .find(|(n, _, _)| *n == "draw-two-sets")
            .expect("the two-set variant")
            .1;
        let words = spirv(source);
        println!("draw-dynamic-offset: {} words, graphics", words.len());
        // SAFETY: as below.
        return unsafe { run_graphics("draw-dynamic-offset", &words, true) };
    }
    if wanted == "draw-many-functions" {
        let source = many_functions();
        let words = spirv(&source);
        println!("draw-many-functions: {} words, graphics", words.len());
        // SAFETY: as below.
        return unsafe { run_graphics("draw-two-sets", &words, true) };
    }
    if let Some((name, source, with_image)) = GRAPHICS.iter().find(|(n, _, _)| *n == wanted) {
        let words = spirv(source);
        println!(
            "{name}: {} words, image bound: {with_image}, graphics",
            words.len()
        );
        // SAFETY: every object is created and destroyed inside, and the queue
        // is idle before anything is dropped.
        return unsafe { run_graphics(name, &words, *with_image) };
    }
    let Some((name, source, with_image)) = VARIANTS.iter().find(|(n, _, _)| *n == wanted) else {
        eprintln!("no variant named {wanted}");
        return std::process::ExitCode::FAILURE;
    };
    let words = spirv(source);
    println!(
        "{name}: {} words, image bound: {with_image}, compute",
        words.len()
    );

    // SAFETY: every object below is created from the one before it, nothing
    // outlives this scope, and the device is idle before anything is dropped.
    unsafe {
        let entry = ash::Entry::load().expect("loading Vulkan");
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .expect("create_instance");
        let physical = *instance
            .enumerate_physical_devices()
            .expect("enumerate")
            .first()
            .expect("a Vulkan device");
        let props = instance.get_physical_device_properties(physical);
        println!(
            "device: {}",
            std::ffi::CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy()
        );
        let family = instance
            .get_physical_device_queue_family_properties(physical)
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .expect("a compute queue") as u32;
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device = instance
            .create_device(
                physical,
                &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
                None,
            )
            .expect("create_device");
        let queue = device.get_device_queue(family, 0);
        let memory_props = instance.get_physical_device_memory_properties(physical);
        let pick = |bits: u32, want: vk::MemoryPropertyFlags| -> u32 {
            (0..memory_props.memory_type_count)
                .find(|i| {
                    bits & (1 << i) != 0
                        && memory_props.memory_types[*i as usize]
                            .property_flags
                            .contains(want)
                })
                .expect("a memory type")
        };

        // The storage buffer every variant writes through.
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(64)
                    .usage(vk::BufferUsageFlags::STORAGE_BUFFER),
                None,
            )
            .expect("create_buffer");
        let need = device.get_buffer_memory_requirements(buffer);
        let buffer_memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(pick(
                        need.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
            .expect("allocate buffer memory");
        device
            .bind_buffer_memory(buffer, buffer_memory, 0)
            .expect("bind_buffer_memory");

        // A 2x2 image, the size the failing renderer test uses.
        let mut image = vk::Image::null();
        let mut image_memory = vk::DeviceMemory::null();
        let mut view = vk::ImageView::null();
        let mut sampler = vk::Sampler::null();
        if *with_image {
            image = device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D {
                            width: 2,
                            height: 2,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .expect("create_image");
            let need = device.get_image_memory_requirements(image);
            image_memory = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(need.size)
                        .memory_type_index(pick(
                            need.memory_type_bits,
                            vk::MemoryPropertyFlags::DEVICE_LOCAL,
                        )),
                    None,
                )
                .expect("allocate image memory");
            device
                .bind_image_memory(image, image_memory, 0)
                .expect("bind_image_memory");
            view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )
                .expect("create_image_view");
            sampler = device
                .create_sampler(&vk::SamplerCreateInfo::default(), None)
                .expect("create_sampler");
        }

        let mut bindings = vec![vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)];
        if *with_image {
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            );
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            );
        }
        let set_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
            .expect("create_descriptor_set_layout");
        let set_layouts = [set_layout];
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
                None,
            )
            .expect("create_pipeline_layout");

        let mut sizes = vec![vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)];
        if *with_image {
            sizes.push(
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1),
            );
            sizes.push(
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1),
            );
        }
        let pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )
            .expect("create_descriptor_pool");
        let set = device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&set_layouts),
            )
            .expect("allocate_descriptor_sets")[0];

        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .range(vk::WHOLE_SIZE)];
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let mut writes = vec![vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info)];
        if *with_image {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_info),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler_info),
            );
        }
        device.update_descriptor_sets(&writes, &[]);

        let module = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
            .expect("create_shader_module");
        let name = std::ffi::CString::new("cs_main").unwrap();
        println!("creating the pipeline");
        let pipelines = device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(
                        vk::PipelineShaderStageCreateInfo::default()
                            .stage(vk::ShaderStageFlags::COMPUTE)
                            .module(module)
                            .name(&name),
                    )
                    .layout(layout)],
                None,
            )
            .expect("create_compute_pipelines");
        println!("pipeline created");

        let command_pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
            .expect("create_command_pool");
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .expect("allocate_command_buffers")[0];
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        if *with_image {
            // Into the layout the descriptor says it is in. Skipping this is
            // invalid usage, and invalid usage segfaults this driver too --
            // which would read exactly like the bug.
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .image(image)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    )],
            );
        }
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipelines[0]);
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            layout,
            0,
            &[set],
            &[],
        );
        println!("recording the dispatch");
        device.cmd_dispatch(cmd, 1, 1, 1);
        println!("dispatch recorded");
        device.end_command_buffer(cmd).expect("end");

        println!("submitting");
        let buffers = [cmd];
        device
            .queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&buffers)],
                vk::Fence::null(),
            )
            .expect("submit");
        device.queue_wait_idle(queue).expect("wait");
        println!("completed");

        device.destroy_command_pool(command_pool, None);
        device.destroy_pipeline(pipelines[0], None);
        device.destroy_shader_module(module, None);
        device.destroy_descriptor_pool(pool, None);
        device.destroy_pipeline_layout(layout, None);
        device.destroy_descriptor_set_layout(set_layout, None);
        if *with_image {
            device.destroy_sampler(sampler, None);
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(image_memory, None);
        }
        device.destroy_buffer(buffer, None);
        device.free_memory(buffer_memory, None);
        device.destroy_device(None);
        instance.destroy_instance(None);
    }
    std::process::ExitCode::SUCCESS
}
