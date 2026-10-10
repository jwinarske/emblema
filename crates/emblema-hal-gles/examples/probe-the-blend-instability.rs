//! The same frame twice in one GLES context, with no renderer in it.
//!
//! `a_blurred_advanced_blend_layer_is_unstable_on_gles` renders one scene
//! twice and gets different bytes the second time. Reduced against four
//! drivers, `radeonsi` is the only one that does it: three levels there, zero
//! on `llvmpipe`, zero on a GC7000UL, zero on an Adreno 640. That is a target
//! and three controls, and this is the reproduction the tree's own rule asks
//! for before a driver is named -- EGL and GL and nothing else.
//!
//! The scene it stands in for is a layer at 0.3 opacity blended `Difference`,
//! holding a mask-blurred rect blended `Multiply`, at four samples. The
//! ingredients that suggests are a multisample target, a draw under an
//! advanced equation, a resolve, and a pass that samples the result -- so each
//! of those is a variant here that can be taken away on its own.
//!
//! ```text
//! probe-the-blend-instability            # every ingredient
//! probe-the-blend-instability --list     # the variants
//! ```
//!
//! A variant prints the worst per-channel difference between the two renders.
//! Zero is stable. Anything else is the defect, and what this is for is a
//! variant set where `radeonsi` is non-zero and the other three are not.

// An example is a program, and it reports by printing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    // `frame` takes the three targets, the coverage texture and the barrier.
    // Bundling them into a struct would be a type whose only job is to have
    // one fewer comma at one call site.
    clippy::too_many_arguments
)]

use glow::HasContext;

/// The target's side, in pixels. Small, because the readback is compared byte
/// for byte and the scene it stands in for is twenty-four across.
const SIDE: i32 = 32;

/// `GL_MULTIPLY_KHR` and `GL_DIFFERENCE_KHR`, which `render.rs` maps the same
/// two blend modes to.
const MULTIPLY_KHR: u32 = 0x9294;
const DIFFERENCE_KHR: u32 = 0x929E;

/// What a variant leaves in.
#[derive(Clone, Copy)]
struct Ingredients {
    /// Samples on the layer's target. One takes the multisample resolve out.
    samples: i32,
    /// Whether the draws inside the layer use an advanced equation.
    advanced_inside: bool,
    /// Whether the composite that samples the layer uses one.
    advanced_composite: bool,
    /// Whether the layer's contents are blurred by sampling a texture.
    blur: bool,
    /// A barrier before every draw and after the resolve, rather than only
    /// before a draw using an advanced equation. If this settles it, the
    /// sequence above was missing one and no driver is at fault.
    extra_barriers: bool,
    /// An ordinary equation set before each advanced one, so the driver sees a
    /// transition rather than a value it already believes is current.
    reset_equation: bool,
    /// One advanced equation for both draws, so the frame never switches
    /// between two of them.
    same_equation: bool,
    /// Clear the layer to mid gray rather than black, so the composite's
    /// source is not zero. With a zero source, the equation, an ordinary
    /// source-over and a discarded source do not all give the same pixel --
    /// but two of the three do, which is how the first reading of this went
    /// wrong.
    gray_layer: bool,
    /// Read the resolved layer back instead of the output, to see whether the
    /// draw inside the layer loses its equation too.
    read_layer: bool,
    /// Which targets get brand-new framebuffers and textures every frame,
    /// never deleted so the names cannot be recycled: bit 1 the multisample
    /// layer, bit 2 the resolve, bit 4 the output. A frame that is correct
    /// with one of these refreshed says the previous frame left its state on
    /// that object.
    fresh: u32,
    /// Delete the targets after every frame and make new ones, so their names
    /// and their memory are recycled -- which is what a renderer freeing a
    /// layer target each frame does, and what `fresh` deliberately does not.
    churn: bool,
    /// A depth-stencil attachment on both targets with the test enabled,
    /// which is what a renderer carrying clip state has.
    stencil: bool,
    /// The advanced draw inside the layer samples a texture through the nine
    /// taps instead of being a solid fill, which is what a mask blur makes of
    /// it: the shape's coverage is blurred and the draw reads it.
    inside_samples: bool,
    /// A second, ordinary draw inside the layer under the advanced one -- the
    /// second child the renderer's reduced scene has, and the one whose
    /// removal makes that scene stable.
    under_draw: bool,
}

const VARIANTS: &[(&str, Ingredients, &str)] = &[
    (
        "full",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "every ingredient the reduced scene has",
    ),
    (
        "single-sample",
        Ingredients {
            samples: 1,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "no multisample target and so no resolve",
    ),
    (
        "no-advanced-inside",
        Ingredients {
            samples: 4,
            advanced_inside: false,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "the layer's own draws blend ordinarily",
    ),
    (
        "no-advanced-composite",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: false,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "the layer is composited ordinarily",
    ),
    (
        "no-advanced",
        Ingredients {
            samples: 4,
            advanced_inside: false,
            advanced_composite: false,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "no advanced equation anywhere -- the control",
    ),
    (
        "barrier-everywhere",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: true,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "the full set, with a barrier before every draw and after the resolve",
    ),
    (
        "re-set-equation",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: true,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "an ordinary equation set before each advanced one",
    ),
    (
        "same-equation",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: true,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "both advanced draws use GL_DIFFERENCE_KHR",
    ),
    (
        "gray-layer",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "a gray layer clear, so the composite's source is not zero",
    ),
    (
        "inspect-layer",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: true,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "the same frame, reading the resolved layer back instead of the output",
    ),
    (
        "fresh-output",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 4,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "a new output every frame, the other two kept -- the one that is clean",
    ),
    (
        "fresh-resolved",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 2,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "a new resolve target every frame, the other two kept",
    ),
    (
        "fresh-layer",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 1,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "a new multisample layer every frame, the other two kept",
    ),
    (
        "churn",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 0,
            churn: true,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "all three deleted and remade every frame, so names and memory recycle",
    ),
    (
        "stencil",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: true,
            inside_samples: false,
            under_draw: false,
        },
        "a depth-stencil attachment with the test enabled",
    ),
    (
        "under-draw",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: true,
        },
        "a second ordinary draw inside the layer, under the advanced one",
    ),
    (
        "renderer-shape",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: true,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: true,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: true,
            under_draw: true,
        },
        "that and a sampling inside draw -- what the renderer's scene needs",
    ),
    (
        "no-blur",
        Ingredients {
            samples: 4,
            advanced_inside: true,
            advanced_composite: true,
            blur: false,
            extra_barriers: false,
            reset_equation: false,
            same_equation: false,
            gray_layer: false,
            read_layer: false,
            fresh: 0,
            churn: false,
            stencil: false,
            inside_samples: false,
            under_draw: false,
        },
        "the layer's contents are not blurred",
    ),
];

/// A quad over `u_rect`, from six indices and no vertex buffer.
const VERTEX: &str = r#"#version 300 es
uniform vec4 u_rect;
void main() {
    vec2 corners[6] = vec2[6](
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
        vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0)
    );
    vec2 t = corners[gl_VertexID];
    gl_Position = vec4(mix(u_rect.xy, u_rect.zw, t), 0.0, 1.0);
}
"#;

/// A solid fill. `BLEND_SUPPORT` is substituted per program, because declaring
/// `blend_support_all_equations` is not free of consequence for a draw that is
/// not using one -- `context.rs` says this driver renders such a draw
/// measurably differently, which is why the renderer keeps two programs.
const SOLID: &str = r#"#version 300 es
EXTENSION
precision highp float;
BLEND_SUPPORT
uniform vec4 u_color;
out vec4 o_color;
void main() { o_color = u_color; }
"#;

/// Nine taps across, which is a mask blur's shape without being one.
const BLUR: &str = r#"#version 300 es
EXTENSION
precision highp float;
BLEND_SUPPORT
uniform sampler2D u_tex;
uniform vec2 u_step;
uniform float u_alpha;
out vec4 o_color;
void main() {
    vec2 size = vec2(textureSize(u_tex, 0));
    vec2 uv = gl_FragCoord.xy / size;
    vec4 sum = vec4(0.0);
    for (int i = -4; i <= 4; i++) {
        sum += texture(u_tex, uv + u_step * float(i));
    }
    o_color = (sum / 9.0) * u_alpha;
}
"#;

/// Samples a texture straight, for the composite with no blur in it.
const SAMPLE: &str = r#"#version 300 es
EXTENSION
precision highp float;
BLEND_SUPPORT
uniform sampler2D u_tex;
uniform float u_alpha;
out vec4 o_color;
void main() {
    vec2 uv = gl_FragCoord.xy / vec2(textureSize(u_tex, 0));
    o_color = texture(u_tex, uv) * u_alpha;
}
"#;

fn program(gl: &glow::Context, fragment: &str, blend_support: bool) -> glow::Program {
    let fragment = fragment
        .replace(
            "EXTENSION",
            if blend_support {
                "#extension GL_KHR_blend_equation_advanced : require"
            } else {
                ""
            },
        )
        .replace(
            "BLEND_SUPPORT",
            if blend_support {
                "layout(blend_support_all_equations) out;"
            } else {
                ""
            },
        );
    // SAFETY: every object below is created, used and deleted on this thread
    // with the context current.
    unsafe {
        let program = gl.create_program().expect("create_program");
        for (stage, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, fragment.as_str()),
        ] {
            let shader = gl.create_shader(stage).expect("create_shader");
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            assert!(
                gl.get_shader_compile_status(shader),
                "compiling {stage:#x}: {}\n{source}",
                gl.get_shader_info_log(shader)
            );
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        assert!(
            gl.get_program_link_status(program),
            "linking: {}",
            gl.get_program_info_log(program)
        );
        program
    }
}

/// A color attachment, multisampled where `samples` is more than one.
struct Target {
    framebuffer: glow::Framebuffer,
    /// The renderbuffers this target owns, so `churn` can delete them.
    owned: Vec<glow::Renderbuffer>,
    /// `None` on a multisample target, which carries a renderbuffer instead
    /// and has to be resolved before anything can sample it.
    texture: Option<glow::Texture>,
}

/// A depth-stencil renderbuffer on the bound framebuffer, where asked.
///
/// Returned rather than forgotten so `churn` can delete it: a target that
/// leaks its attachments is not a target that was remade.
fn attach_stencil(gl: &glow::Context, samples: i32, with_stencil: bool) -> Vec<glow::Renderbuffer> {
    if !with_stencil {
        return Vec::new();
    }
    // SAFETY: as elsewhere in this file -- a current context on this thread.
    unsafe {
        let rb = gl.create_renderbuffer().expect("create_renderbuffer");
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
        if samples > 1 {
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::DEPTH24_STENCIL8,
                SIDE,
                SIDE,
            );
        } else {
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, SIDE, SIDE);
        }
        gl.framebuffer_renderbuffer(
            glow::FRAMEBUFFER,
            glow::DEPTH_STENCIL_ATTACHMENT,
            glow::RENDERBUFFER,
            Some(rb),
        );
        vec![rb]
    }
}

fn target(gl: &glow::Context, samples: i32, with_stencil: bool) -> Target {
    // SAFETY: as above.
    unsafe {
        let framebuffer = gl.create_framebuffer().expect("create_framebuffer");
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        if samples > 1 {
            let rb = gl.create_renderbuffer().expect("create_renderbuffer");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::RGBA8,
                SIDE,
                SIDE,
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(rb),
            );
            let mut owned = vec![rb];
            owned.extend(attach_stencil(gl, samples, with_stencil));
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "the multisample target is incomplete at {samples} samples"
            );
            Target {
                framebuffer,
                owned,
                texture: None,
            }
        } else {
            let texture = gl.create_texture().expect("create_texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                SIDE,
                SIDE,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                None,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let owned = attach_stencil(gl, 1, with_stencil);
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "the single-sample target is incomplete"
            );
            Target {
                framebuffer,
                owned,
                texture: Some(texture),
            }
        }
    }
}

struct Programs {
    solid_plain: glow::Program,
    solid_blend: glow::Program,
    blur_plain: glow::Program,
    blur_blend: glow::Program,
    sample_plain: glow::Program,
    sample_blend: glow::Program,
}

/// One frame: fill the layer, blur it into the layer's target under an
/// advanced equation, resolve, then composite onto the output under another.
///
/// Returns the output's bytes.
fn frame(
    gl: &glow::Context,
    programs: &Programs,
    what: Ingredients,
    layer: &Target,
    resolved: &Target,
    output: &Target,
    blend_barrier: Option<extern "C" fn()>,
    coverage: Option<glow::Texture>,
) -> Vec<u8> {
    // SAFETY: as above.
    unsafe {
        gl.viewport(0, 0, SIDE, SIDE);
        if what.stencil {
            // Enabled and always passing: the point is that the state is on,
            // as a renderer's clip machinery leaves it, not that it clips.
            gl.enable(glow::STENCIL_TEST);
            gl.stencil_func(glow::ALWAYS, 0, 0xff);
            gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
        } else {
            gl.disable(glow::STENCIL_TEST);
        }

        // The layer's own contents: an opaque rect, then a second one over it
        // under `Multiply`, which is what the scene's blurred draw blends with.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(layer.framebuffer));
        gl.disable(glow::BLEND);
        if what.gray_layer {
            gl.clear_color(0.5, 0.5, 0.5, 1.0);
        } else {
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
        }
        gl.clear(glow::COLOR_BUFFER_BIT);

        // The second child first, as the renderer's scene has it: an ordinary
        // draw under the advanced one, which gives it a destination to read.
        if what.under_draw {
            gl.use_program(Some(programs.solid_plain));
            gl.uniform_4_f32(
                gl.get_uniform_location(programs.solid_plain, "u_rect")
                    .as_ref(),
                -0.84,
                -0.84,
                0.12,
                0.12,
            );
            gl.uniform_4_f32(
                gl.get_uniform_location(programs.solid_plain, "u_color")
                    .as_ref(),
                0.0,
                0.0,
                0.0,
                1.0,
            );
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }

        let under = if what.inside_samples {
            let program = if what.advanced_inside {
                programs.blur_blend
            } else {
                programs.blur_plain
            };
            gl.use_program(Some(program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, coverage);
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_tex").as_ref(), 0);
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_alpha").as_ref(), 1.0);
            gl.uniform_2_f32(
                gl.get_uniform_location(program, "u_step").as_ref(),
                1.0 / SIDE as f32,
                0.0,
            );
            program
        } else {
            let program = if what.advanced_inside {
                programs.solid_blend
            } else {
                programs.solid_plain
            };
            gl.use_program(Some(program));
            program
        };
        gl.uniform_4_f32(
            gl.get_uniform_location(under, "u_rect").as_ref(),
            -0.84,
            -0.84,
            0.5,
            0.5,
        );
        if !what.inside_samples {
            gl.uniform_4_f32(
                gl.get_uniform_location(under, "u_color").as_ref(),
                0.0,
                0.221_247,
                0.0,
                1.0,
            );
        }
        gl.enable(glow::BLEND);
        if what.advanced_inside {
            if what.reset_equation {
                gl.blend_equation(glow::FUNC_ADD);
            }
            gl.blend_equation(if what.same_equation {
                DIFFERENCE_KHR
            } else {
                MULTIPLY_KHR
            });
            if let Some(barrier) = blend_barrier {
                barrier();
            }
        } else {
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
        if what.extra_barriers {
            if let Some(barrier) = blend_barrier {
                barrier();
            }
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        if what.extra_barriers {
            if let Some(barrier) = blend_barrier {
                barrier();
            }
        }

        // Resolve, where there is anything to resolve.
        let sampled = if what.samples > 1 {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(layer.framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(resolved.framebuffer));
            gl.blit_framebuffer(
                0,
                0,
                SIDE,
                SIDE,
                0,
                0,
                SIDE,
                SIDE,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            if what.extra_barriers {
                if let Some(barrier) = blend_barrier {
                    barrier();
                }
            }
            resolved.texture.expect("the resolve target is a texture")
        } else {
            layer.texture.expect("a single-sample layer is a texture")
        };

        // The composite: sample the layer and blend it onto the output.
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(output.framebuffer));
        gl.disable(glow::BLEND);
        gl.clear_color(0.1, 0.1, 0.1, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);

        let composite = match (what.blur, what.advanced_composite) {
            (true, true) => programs.blur_blend,
            (true, false) => programs.blur_plain,
            (false, true) => programs.sample_blend,
            (false, false) => programs.sample_plain,
        };
        gl.use_program(Some(composite));
        gl.uniform_4_f32(
            gl.get_uniform_location(composite, "u_rect").as_ref(),
            -1.0,
            -1.0,
            1.0,
            1.0,
        );
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(sampled));
        gl.uniform_1_i32(gl.get_uniform_location(composite, "u_tex").as_ref(), 0);
        gl.uniform_1_f32(gl.get_uniform_location(composite, "u_alpha").as_ref(), 0.3);
        if what.blur {
            gl.uniform_2_f32(
                gl.get_uniform_location(composite, "u_step").as_ref(),
                1.0 / SIDE as f32,
                0.0,
            );
        }
        gl.enable(glow::BLEND);
        if what.advanced_composite {
            if what.reset_equation {
                gl.blend_equation(glow::FUNC_ADD);
            }
            gl.blend_equation(DIFFERENCE_KHR);
            if let Some(barrier) = blend_barrier {
                barrier();
            }
        } else {
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
        if what.extra_barriers {
            if let Some(barrier) = blend_barrier {
                barrier();
            }
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);

        // GLES has no validation layer, so this is the whole of what can be
        // asked cheaply: a driver that minded any of the above would have set
        // an error. It is not proof of valid usage, and nothing here treats it
        // as such.
        let error = gl.get_error();
        assert_eq!(
            error,
            glow::NO_ERROR,
            "the driver reported {error:#x} for this sequence, so it is not a finding"
        );

        let mut pixels = vec![0u8; (SIDE * SIDE * 4) as usize];
        // The resolved layer where asked, so the draw inside the layer can be
        // read on its own; the output otherwise.
        gl.bind_framebuffer(
            glow::READ_FRAMEBUFFER,
            Some(if what.read_layer {
                resolved.framebuffer
            } else {
                output.framebuffer
            }),
        );
        gl.read_pixels(
            0,
            0,
            SIDE,
            SIDE,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut pixels),
        );
        pixels
    }
}

/// `EGL_PLATFORM_SURFACELESS_MESA`. The target and its first control are both
/// Mesa drivers and reach the display this way.
const PLATFORM_SURFACELESS: khronos_egl::Enum = 0x31DD;

/// `EGL_PLATFORM_GBM_KHR`, which the two vendor stacks need: a GC7000UL and an
/// Adreno ship `libgbm` and advertise the GBM platform while having no
/// surfaceless *platform* at all. Four drivers is the whole point of this
/// probe, so it reaches a display both ways.
const PLATFORM_GBM: khronos_egl::Enum = 0x31D7;

/// `libgbm`, opened at run time rather than linked, so this cross-builds to a
/// board without the library on the build host. Two symbols are all a display
/// needs; surfaces belong to a presentation path and this renders into
/// framebuffer objects.
struct Gbm {
    _library: libloading::Library,
    device: *mut std::ffi::c_void,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    _node: std::fs::File,
}

impl Gbm {
    /// The first render node that yields a device, or `None`.
    fn open() -> Option<Self> {
        let nodes: Vec<String> = match std::env::var("EMBLEMA_GBM_NODE") {
            Ok(named) => vec![named],
            Err(_) => {
                let mut found: Vec<String> = std::fs::read_dir("/dev/dri")
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("renderD"))
                    .map(|n| format!("/dev/dri/{n}"))
                    .collect();
                found.sort();
                found
            }
        };
        // SAFETY: the symbols below are `libgbm`'s documented C entry points,
        // and the library outlives the device it creates.
        unsafe {
            let library = libloading::Library::new("libgbm.so.1").ok()?;
            let create: libloading::Symbol<
                unsafe extern "C" fn(std::os::fd::RawFd) -> *mut std::ffi::c_void,
            > = library.get(b"gbm_create_device\0").ok()?;
            let destroy: libloading::Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> =
                library.get(b"gbm_device_destroy\0").ok()?;
            let destroy = *destroy;
            for node in nodes {
                let Ok(file) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&node)
                else {
                    continue;
                };
                let device = create(std::os::fd::AsRawFd::as_raw_fd(&file));
                if !device.is_null() {
                    println!("gbm device: {node}");
                    return Some(Self {
                        _library: library,
                        device,
                        destroy,
                        _node: file,
                    });
                }
            }
            None
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // SAFETY: the device came from `gbm_create_device` and is destroyed
        // once.
        unsafe { (self.destroy)(self.device) };
    }
}

fn main() {
    let wanted = std::env::args().nth(1);
    if wanted.as_deref() == Some("--list") {
        println!("variants:");
        for (name, _, what) in VARIANTS {
            println!("  {name:24} {what}");
        }
        return;
    }

    // SAFETY: the EGL library, display and context outlive every call below,
    // and the context is current on this thread for all of the GL.
    unsafe {
        let egl = khronos_egl::DynamicInstance::<khronos_egl::EGL1_5>::load_required()
            .expect("loading libEGL");
        let client_extensions = egl
            .query_string(None, khronos_egl::EXTENSIONS)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Surfaceless where it exists, GBM otherwise. Held in scope either
        // way: the device has to outlive the display taken from it.
        let mut gbm = None;
        let display = if client_extensions.contains("EGL_MESA_platform_surfaceless") {
            println!("platform: surfaceless");
            egl.get_platform_display(
                PLATFORM_SURFACELESS,
                khronos_egl::DEFAULT_DISPLAY,
                &[khronos_egl::ATTRIB_NONE],
            )
            .expect("get_platform_display")
        } else if client_extensions.contains("EGL_KHR_platform_gbm")
            || client_extensions.contains("EGL_MESA_platform_gbm")
        {
            println!("platform: gbm");
            let device = Gbm::open().expect("a GBM device from a render node");
            let native = device.device;
            gbm = Some(device);
            egl.get_platform_display(PLATFORM_GBM, native, &[khronos_egl::ATTRIB_NONE])
                .expect("get_platform_display (gbm)")
        } else {
            println!("skipping: neither the surfaceless nor the GBM platform");
            return;
        };
        egl.initialize(display).expect("initialize");
        egl.bind_api(khronos_egl::OPENGL_ES_API).expect("bind_api");
        let config = egl
            .choose_first_config(
                display,
                &[
                    khronos_egl::SURFACE_TYPE,
                    // The two platforms offer different surface types, and
                    // asking GBM for a pbuffer matches no config at all --
                    // which reads as "this driver has no ES3 RGBA8" and is
                    // nothing of the kind.
                    if gbm.is_some() {
                        khronos_egl::WINDOW_BIT
                    } else {
                        khronos_egl::PBUFFER_BIT
                    },
                    khronos_egl::RENDERABLE_TYPE,
                    khronos_egl::OPENGL_ES3_BIT,
                    khronos_egl::RED_SIZE,
                    8,
                    khronos_egl::GREEN_SIZE,
                    8,
                    khronos_egl::BLUE_SIZE,
                    8,
                    khronos_egl::ALPHA_SIZE,
                    8,
                    khronos_egl::NONE,
                ],
            )
            .expect("choose_first_config")
            .expect("an ES3 RGBA8 config");
        let context = egl
            .create_context(
                display,
                config,
                None,
                &[
                    khronos_egl::CONTEXT_MAJOR_VERSION,
                    3,
                    khronos_egl::CONTEXT_MINOR_VERSION,
                    0,
                    khronos_egl::NONE,
                ],
            )
            .expect("create_context");
        egl.make_current(display, None, None, Some(context))
            .expect("make_current");

        let gl = glow::Context::from_loader_function(|name| {
            egl.get_proc_address(name)
                .map_or(std::ptr::null(), |p| p as *const std::ffi::c_void)
        });

        println!("renderer: {}", gl.get_parameter_string(glow::RENDERER));
        println!("version:  {}", gl.get_parameter_string(glow::VERSION));
        let extensions = gl.get_parameter_string(glow::EXTENSIONS);
        let advanced = extensions.contains("GL_KHR_blend_equation_advanced");
        let coherent = extensions.contains("GL_KHR_blend_equation_advanced_coherent");
        if !advanced {
            println!("skipping: no GL_KHR_blend_equation_advanced");
            return;
        }
        println!("advanced blending: yes, coherent: {coherent}");

        // Without the coherent variant, overlapping draws under an advanced
        // equation need this between them.
        let blend_barrier: Option<extern "C" fn()> = if coherent {
            None
        } else {
            egl.get_proc_address("glBlendBarrierKHR")
                .map(|p| std::mem::transmute::<_, extern "C" fn()>(p))
        };
        if !coherent && blend_barrier.is_none() {
            println!("skipping: non-coherent advanced blending and no glBlendBarrierKHR");
            return;
        }

        // ES 3 core wants a vertex array bound even for a draw with no
        // attributes.
        let vao = gl.create_vertex_array().expect("create_vertex_array");
        gl.bind_vertex_array(Some(vao));

        let programs = Programs {
            solid_plain: program(&gl, SOLID, false),
            solid_blend: program(&gl, SOLID, true),
            blur_plain: program(&gl, BLUR, false),
            blur_blend: program(&gl, BLUR, true),
            sample_plain: program(&gl, SAMPLE, false),
            sample_blend: program(&gl, SAMPLE, true),
        };

        let chosen: Vec<_> = match wanted {
            None => VARIANTS.iter().collect(),
            Some(name) => {
                let found: Vec<_> = VARIANTS.iter().filter(|(n, _, _)| *n == name).collect();
                assert!(!found.is_empty(), "no variant named {name}; try --list");
                found
            }
        };

        // Something for an inside draw to sample: a texture filled once and
        // never touched again, standing in for a blurred coverage.
        let cov = target(&gl, 1, false);
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(cov.framebuffer));
        gl.disable(glow::BLEND);
        gl.clear_color(0.75, 0.5, 0.25, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        let coverage = cov.texture;

        println!();
        for (name, what, _) in chosen {
            let layer = target(&gl, what.samples, what.stencil);
            let resolved = target(&gl, 1, what.stencil);
            let output = target(&gl, 1, what.stencil);

            // `RUNS` renders more than twice and reports each frame against
            // the first and the one before it, with the center pixel. Two
            // frames cannot tell a first-frame effect from a drift, and a
            // difference cannot say which frame is the correct one.
            let runs: usize = std::env::var("RUNS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2)
                .max(2);
            let center = |pixels: &[u8]| {
                let at = ((16 * SIDE as usize) + 16) * 4;
                format!(
                    "[{:3},{:3},{:3},{:3}]",
                    pixels[at],
                    pixels[at + 1],
                    pixels[at + 2],
                    pixels[at + 3]
                )
            };
            // Rebuilt rather than reused where the variant asks, and never
            // deleted, so a name cannot come back and bring its state with it.
            let mut layer = layer;
            let mut resolved = resolved;
            let mut output = output;
            let render = |layer: &mut Target, resolved: &mut Target, output: &mut Target| {
                if what.churn {
                    for t in [&mut *layer, &mut *resolved, &mut *output] {
                        gl.delete_framebuffer(t.framebuffer);
                        if let Some(texture) = t.texture {
                            gl.delete_texture(texture);
                        }
                        for rb in std::mem::take(&mut t.owned) {
                            gl.delete_renderbuffer(rb);
                        }
                    }
                    *layer = target(&gl, what.samples, what.stencil);
                    *resolved = target(&gl, 1, what.stencil);
                    *output = target(&gl, 1, what.stencil);
                }
                if what.fresh & 1 != 0 {
                    *layer = target(&gl, what.samples, what.stencil);
                }
                if what.fresh & 2 != 0 {
                    *resolved = target(&gl, 1, what.stencil);
                }
                if what.fresh & 4 != 0 {
                    *output = target(&gl, 1, what.stencil);
                }
                frame(
                    &gl,
                    &programs,
                    *what,
                    layer,
                    resolved,
                    output,
                    blend_barrier,
                    coverage,
                )
            };

            let first = render(&mut layer, &mut resolved, &mut output);
            if runs > 2 {
                println!("{name:24} frame  1: {:>40} | center {}", "", center(&first));
            }
            let mut previous = first.clone();
            let mut worst = 0;
            let mut differing = 0;
            for run in 2..=runs {
                let current = render(&mut layer, &mut resolved, &mut output);
                let worst_first = first
                    .iter()
                    .zip(current.iter())
                    .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                    .max()
                    .unwrap_or(0);
                let differ_first = first
                    .iter()
                    .zip(current.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                let worst_previous = previous
                    .iter()
                    .zip(current.iter())
                    .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                    .max()
                    .unwrap_or(0);
                if runs > 2 {
                    println!(
                        "{name:24} frame {run:2}: vs first {worst_first:3} \
                         ({differ_first:5} byte(s)), vs previous \
                         {worst_previous:3} | center {}",
                        center(&current)
                    );
                }
                if run == 2 {
                    worst = worst_first;
                    differing = differ_first;
                }
                previous = current;
            }
            if runs == 2 {
                println!(
                    "{name:24} worst {worst:3} level(s), {differing:5} byte(s) of {} differ",
                    first.len()
                );
            }

            gl.delete_framebuffer(layer.framebuffer);
            gl.delete_framebuffer(resolved.framebuffer);
            gl.delete_framebuffer(output.framebuffer);
        }

        egl.destroy_context(display, context)
            .expect("destroy_context");
        egl.terminate(display).expect("terminate");
        drop(gbm);
    }
}
