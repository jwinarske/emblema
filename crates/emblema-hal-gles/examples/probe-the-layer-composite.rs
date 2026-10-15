//! The second instability's frame loop, in a bare GL program.
//!
//! `a_generated_scene_survives_other_frames` renders a subject, then three
//! other frames, then the subject again, and gets different bytes the second
//! time. `ablate-the-frame-loop` reduced it to five necessary ingredients, and
//! `probe-the-blend-instability` -- the first instability's bare probe -- cannot
//! express it: it reads zero in every shape, with the right equation, the right
//! arrangement and the right inlined variant. What that left is the renderer's
//! *shader*, which is `SOLID_FS_GLSL` with Mesa's own
//! `KHR_blend_equation_advanced` lowering appended to it, and the renderer's
//! *pass structure*, which is four passes rather than three.
//!
//! So this is the frame loop hand-written against EGL and GL, with the
//! renderer's own program compiled into it and its paint block filled in by
//! hand. Nothing from this renderer runs: the shader source is a string, and
//! every pass, target, buffer and draw below is written out.
//!
//! It reproduces. Eleven hits of twelve on `radeonsi` against zero on
//! `llvmpipe`, the same two pixel values the renderer reads at its worst pixel,
//! and the renderer's ablation table row for row. And `small-shader` -- the same
//! four passes with a thirty-line fragment program instead of the generated one
//! -- reads zero of twelve, which is what says the **shader body** is the
//! ingredient rather than the arrangement.
//!
//! # What the renderer does, measured rather than guessed
//!
//! The subject's recording is four single-sample passes over a 128 by 128
//! frame:
//!
//! | pass | what |
//! |---|---|
//! | 0 | the layer's own content: two solid draws over a transparent clear |
//! | 1 | the first half of the layer blur, sampling pass 0, `Src` |
//! | 2 | the second half, sampling pass 1, `Src` |
//! | 3 | the frame: a solid draw, then pass 2 composited as an image under `GL_OVERLAY_KHR` at an alpha of 0.68 |
//!
//! and the three other frames are one single-sample pair of solid draws, one
//! four-sample pair, and one four-sample pass holding a draw under an advanced
//! equation which a second pass composites. The blur passes are not decoration:
//! `subject-no-layer-blur` reads zero of twelve in the renderer, and so do the
//! others at one sample and the others without an advanced equation.
//!
//! ```text
//! probe-the-layer-composite            # every ingredient
//! probe-the-layer-composite --list     # the variants
//! probe-the-layer-composite --shader   # the fragment program, as the driver gets it
//! ```
//!
//! `EQUATION` and `OTHERS_EQUATION` name the advanced equation each side
//! blends with, as `SUBJECT_MODE` and `OTHERS_MODE` do on the reducer -- the
//! subject's is the one that matters, and of the eleven only `overlay` and
//! `hardlight` reproduce there. `RUNS=n` repeats the whole loop, because the
//! defect is intermittent: eleven hits of twelve here, ten in the renderer.
//! `MAP=1` draws the differing pixels and `AT=x,y` prints one from both renders.
//!
//! A variant prints the worst per-channel difference between the subject's two
//! renders. Zero is stable.

// An example is a program, and it reports by printing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    // Every rectangle, color and mapping below is one the renderer's own
    // recording was measured to carry. Naming each of them would be naming
    // the shrunk case's arbitrary numbers twice.
    clippy::unreadable_literal
)]

mod bare;

use glow::HasContext;

/// The frame's side, in pixels, which is the reducer's own.
const SIDE: i32 = 128;

/// `GL_OVERLAY_KHR`, the subject's equation and the one that matters.
const OVERLAY_KHR: u32 = 0x9296;
/// `GL_DIFFERENCE_KHR`, which the other frames reach.
const DIFFERENCE_KHR: u32 = 0x929E;

/// How many texture units a draw binds.
///
/// `emblema_hal::MAX_EFFECT_TEXTURES`, and the renderer binds every one of them
/// for every draw rather than only the ones a draw samples -- a unit left
/// pointing at a deleted texture is what that avoids. Stated here because this
/// file reproduces the GL stream and not the renderer's reasons.
const UNITS: u32 = 4;

/// The paint block's floats, and where each thing sits in it.
///
/// `emblema_hal::material::layout`, restated rather than imported: this probe
/// is the artifact a driver bug report carries, and a reader of it cannot
/// follow a constant into a crate they do not have. Sixteen four-component
/// vectors, which is what the shader declares.
const MATERIAL_FLOATS: usize = 64;
const STOPS: usize = 0;
const GEOMETRY: usize = 20;
const TO_LOCAL: usize = 24;
const PARAMS: usize = 36;
const FILTER_PARAMS: usize = 60;

/// The material kinds the shader dispatches on, of which this frame uses three.
const KIND_SOLID: f32 = 0.0;
const KIND_IMAGE: f32 = 4.0;
const KIND_BLUR: f32 = 6.0;

/// `BlendMode::Modulate`, which is the tint blend every draw carries by default
/// and the identity against the opaque white the vertices hold.
const TINT_MODULATE: f32 = 13.0;

/// The whole of the target, mapped to the whole of the texture it samples.
///
/// Three columns of a three-by-three in column order, each padded to four, as
/// the block declares it. These are the numbers the renderer's own recording
/// carries for an unbounded layer composited at its own size: clip x and y into
/// zero-to-one, with y turned over because the shader negates it on the way
/// out.
const WHOLE: [f32; 12] = [0.5, 0.0, 0.0, 0.0, 0.0, -0.5, 0.0, 0.0, 0.5, 0.5, 1.0, 0.0];

/// The same three materials in thirty lines, as the control on the shader.
///
/// `as-found` differs from `probe-the-blend-instability` in two things at once:
/// the renderer's fragment program and the renderer's four passes. This variant
/// changes exactly one of them. The interface is the generated shader's --
/// the same block, the same instance name, the same varyings and the same
/// sampler -- so the only thing that moves is the body the driver compiles and
/// the lowering is appended to.
///
/// It draws a slightly different picture: nine taps at a fixed spread rather
/// than the tap count the deviation implies, and no color filter or dither path
/// at all. That is what "a short program" means, and the question it answers is
/// whether a short one in this arrangement reproduces.
const SMALL_FS: &str = r#"#version 300 es

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
layout(std140) uniform Paint_block_0Fragment { Paint _group_1_binding_0_fs; };

uniform highp sampler2D _group_0_binding_0_fs;

smooth in vec3 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
smooth in vec4 _vs2fs_location2;
layout(location = 0) out vec4 _fs2p_location0;

vec2 to_local_space(vec3 clip) {
    Paint p = _group_1_binding_0_fs;
    vec3 mapped = p.to_local[0].xyz * clip.x
                + p.to_local[1].xyz * clip.y
                + p.to_local[2].xyz * clip.z;
    return mapped.xy / max(mapped.z, 1e-6);
}

void main() {
    Paint p = _group_1_binding_0_fs;
    float kind = p.params.y;
    vec4 color;
    if (kind < 0.5) {
        vec4 c = p.stops[0];
        color = vec4(c.rgb * c.a, c.a);
    } else if (kind > 5.5 && kind < 6.5) {
        vec2 uv = to_local_space(_vs2fs_location0);
        vec2 step = p.geometry.zw;
        float sigma = max(p.params.z, 1e-6);
        vec4 sum = vec4(0.0);
        float total = 0.0;
        for (int i = -4; i <= 4; i++) {
            float w = exp(-0.5 * float(i * i) / (sigma * sigma));
            sum += texture(_group_0_binding_0_fs, uv + step * float(i)) * w;
            total += w;
        }
        color = sum / total;
    } else {
        vec2 uv = clamp(to_local_space(_vs2fs_location0), 0.0, 1.0);
        color = texture(_group_0_binding_0_fs, uv) * p.geometry.z;
    }
    _fs2p_location0 = _vs2fs_location2 * color;
}
"#;

/// What a draw is filled with.
#[derive(Clone, Copy)]
enum Material {
    /// A color, straight rather than premultiplied: the shader premultiplies.
    Solid([f32; 4]),
    /// One axis of a layer blur, sampling the pass before it.
    Blur {
        to_local: [f32; 12],
        /// One tap's step in the sampled texture's coordinates, so a horizontal
        /// pass over a target `w` wide steps `(1/w, 0)`.
        step: [f32; 2],
        /// Standard deviation in taps, which is what decides the tap count --
        /// the loop the shader runs, and the reason this material is not a
        /// handful of instructions.
        sigma: f32,
    },
    /// A finished pass drawn as a texture, which is what compositing a layer is.
    Image { to_local: [f32; 12], alpha: f32 },
}

impl Material {
    /// The block the shader reads, packed as `Material::to_uniform` packs it.
    fn pack(self) -> [f32; MATERIAL_FLOATS] {
        let mut out = [0.0f32; MATERIAL_FLOATS];
        // Every draw carries these two: one stop in use, and the tint blend
        // the vertex color goes through.
        out[PARAMS] = 1.0;
        out[FILTER_PARAMS + 1] = TINT_MODULATE;
        match self {
            Self::Solid(color) => {
                out[STOPS..STOPS + 4].copy_from_slice(&color);
                out[PARAMS + 1] = KIND_SOLID;
            }
            Self::Blur {
                to_local,
                step,
                sigma,
            } => {
                out[GEOMETRY + 2] = step[0];
                out[GEOMETRY + 3] = step[1];
                out[TO_LOCAL..TO_LOCAL + 12].copy_from_slice(&to_local);
                out[PARAMS + 1] = KIND_BLUR;
                out[PARAMS + 2] = sigma;
            }
            Self::Image { to_local, alpha } => {
                // The source rectangle, which is the whole texture, and the
                // tint, which is white and opaque and therefore the identity.
                out[STOPS..STOPS + 4].copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
                out[STOPS + 4..STOPS + 8].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                out[GEOMETRY + 2] = alpha;
                // The tile mode, clamped, and the sampling, linear -- both zero
                // and both written out, because a reader of this file should not
                // have to know that the kinds they want happen to be the zeroth.
                out[GEOMETRY + 3] = 0.0;
                out[TO_LOCAL..TO_LOCAL + 12].copy_from_slice(&to_local);
                out[PARAMS + 1] = KIND_IMAGE;
                out[PARAMS + 2] = 0.0;
            }
        }
        out
    }
}

/// What the blend unit does with a draw.
#[derive(Clone, Copy, PartialEq)]
enum Blend {
    /// The source outright, which needs no blend unit at all.
    Src,
    /// Premultiplied source-over.
    Over,
    /// One of the extension's equations, which the blend-qualified program and
    /// a barrier before the draw go with.
    Advanced(u32),
}

/// One draw: a rectangle, a material, a blend, and what it samples.
#[derive(Clone, Copy)]
struct Draw {
    /// In pixels of the pass's own target, as the shrunk case states them.
    rect: [f32; 4],
    material: Material,
    blend: Blend,
    /// Which earlier target this draw samples, by its index in the frame, or
    /// `None` for a draw that samples nothing and binds the placeholder.
    samples: Option<usize>,
}

/// One pass: a target, a clear, and the draws that go into it.
struct Pass {
    extent: (i32, i32),
    samples: i32,
    clear: [f32; 4],
    draws: Vec<Draw>,
}

/// What a variant leaves in, named as `ablate-the-frame-loop`'s fields are so
/// the two tables can be read side by side.
#[derive(Clone, Copy)]
struct Shape {
    /// Samples on the subject's passes. The renderer reads zero of twelve at
    /// four, which is the opposite of the first instability.
    subject_samples: i32,
    /// The subject's layer carries a blur, which is two passes of its own.
    subject_blur: bool,
    /// The layer's composite blends under an advanced equation.
    subject_advanced: bool,
    /// Samples on the other frames' passes.
    others_samples: i32,
    /// The other frames reach an advanced equation.
    others_advanced: bool,
    /// Draw with the renderer's own fragment program rather than the short one.
    renderer_shader: bool,
}

const AS_FOUND: Shape = Shape {
    subject_samples: 1,
    subject_blur: true,
    subject_advanced: true,
    others_samples: 4,
    others_advanced: true,
    renderer_shader: true,
};

const VARIANTS: &[(&str, Shape, &str)] = &[
    ("as-found", AS_FOUND, "every ingredient the renderer needs"),
    (
        "subject-multisampled",
        Shape {
            subject_samples: 4,
            ..AS_FOUND
        },
        "the subject's passes at four samples",
    ),
    (
        "no-layer-blur",
        Shape {
            subject_blur: false,
            ..AS_FOUND
        },
        "the two blur passes taken out, so the layer composites unblurred",
    ),
    (
        "subject-no-advanced",
        Shape {
            subject_advanced: false,
            ..AS_FOUND
        },
        "the layer composited source-over",
    ),
    (
        "others-single-sample",
        Shape {
            others_samples: 1,
            ..AS_FOUND
        },
        "the other frames at one sample",
    ),
    (
        "others-no-advanced",
        Shape {
            others_advanced: false,
            ..AS_FOUND
        },
        "the other frames blending ordinarily",
    ),
    (
        "small-shader",
        Shape {
            renderer_shader: false,
            ..AS_FOUND
        },
        "the same four passes with a short fragment program instead of the \
         renderer's -- the control that separates the shader from the arrangement",
    ),
];

/// Which advanced equation a side blends with, from the environment.
fn equation(var: &str, fallback: u32) -> u32 {
    match std::env::var(var).unwrap_or_default().as_str() {
        "multiply" => 0x9294,
        "screen" => 0x9295,
        "overlay" => OVERLAY_KHR,
        "darken" => 0x9297,
        "lighten" => 0x9298,
        "colordodge" => 0x9299,
        "colorburn" => 0x929A,
        "hardlight" => 0x929B,
        "softlight" => 0x929C,
        "difference" => DIFFERENCE_KHR,
        "exclusion" => 0x92A0,
        _ => fallback,
    }
}

/// The subject: the layer's content, its blur in two passes, and the frame that
/// composites it.
///
/// Every rectangle, color, alpha and sigma here is one the renderer's own
/// recording of the shrunk case was measured to carry. The clip masks it also
/// carries are left out: `no-clips` reads ten of twelve there, so they are not
/// an ingredient, and a scissor and a stencil would be two more things in a file
/// whose job is to be minimal.
fn subject(what: Shape) -> Vec<Pass> {
    let composite = if what.subject_advanced {
        Blend::Advanced(equation("EQUATION", OVERLAY_KHR))
    } else {
        Blend::Over
    };
    let step = 1.0 / SIDE as f32;
    let sigma = 3.1289234;
    let whole = [0.0, 0.0, SIDE as f32, SIDE as f32];

    // The layer's own two draws, over a transparent clear.
    let content = Pass {
        extent: (SIDE, SIDE),
        samples: what.subject_samples,
        clear: [0.0, 0.0, 0.0, 0.0],
        draws: vec![
            Draw {
                rect: [31.0, 21.0, 62.0, 42.0],
                material: Material::Solid([0.82257867, 0.27902827, 0.66923934, 1.0]),
                blend: Blend::Over,
                samples: None,
            },
            Draw {
                rect: [10.0, 23.0, 44.0, 64.0],
                material: Material::Solid([0.1675554, 0.4423193, 0.4975624, 0.77261364]),
                blend: Blend::Over,
                samples: None,
            },
        ],
    };

    // One axis each, which is what makes a blur two passes: the kernel is
    // separable, so blurring along each axis in turn gives the square of taps
    // at the cost of twice the line.
    let mut passes = vec![content];
    if what.subject_blur {
        for axis in [[step, 0.0], [0.0, step]] {
            let source = passes.len() - 1;
            passes.push(Pass {
                extent: (SIDE, SIDE),
                samples: what.subject_samples,
                clear: [0.0, 0.0, 0.0, 0.0],
                draws: vec![Draw {
                    rect: whole,
                    material: Material::Blur {
                        to_local: WHOLE,
                        step: axis,
                        sigma,
                    },
                    blend: Blend::Src,
                    samples: Some(source),
                }],
            });
        }
    }

    let layer = passes.len() - 1;
    passes.push(Pass {
        extent: (SIDE, SIDE),
        samples: what.subject_samples,
        clear: [0.0, 0.0, 0.0, 1.0],
        draws: vec![
            Draw {
                rect: [6.0, 26.0, 49.0, 43.0],
                material: Material::Solid([0.5475806, 0.3393603, 0.1716616, 1.0]),
                blend: Blend::Over,
                samples: None,
            },
            // The composite, and the whole subject of this file: the finished
            // layer drawn as a texture, under the equation, at the layer's own
            // opacity.
            Draw {
                rect: whole,
                material: Material::Image {
                    to_local: WHOLE,
                    alpha: 0.6805242,
                },
                blend: composite,
                samples: Some(layer),
            },
        ],
    });
    passes
}

/// The three frames between the subject's two renders.
///
/// Reduced to one at first, which did not reproduce in the renderer, so the
/// list is kept whole and what varies is the ingredients. What they owe is an
/// advanced blend and more than one sample; the mask blur one of them carries
/// is not an ingredient -- `others-no-mask-blur` reads ten of twelve -- so it
/// is left out here.
fn others(what: Shape) -> Vec<Vec<Pass>> {
    let advanced = if what.others_advanced {
        Blend::Advanced(equation("OTHERS_EQUATION", DIFFERENCE_KHR))
    } else {
        Blend::Over
    };
    vec![
        // The first: ordinary, at one sample whatever the variant says, with a
        // `Src` draw in it. The renderer's own first frame is this.
        vec![Pass {
            extent: (SIDE, SIDE),
            samples: 1,
            clear: [0.0, 0.0, 0.0, 1.0],
            draws: vec![
                Draw {
                    rect: [31.0, 17.0, 54.0, 62.0],
                    material: Material::Solid([0.34063724, 0.09357163, 0.6587454, 0.22730592]),
                    blend: Blend::Over,
                    samples: None,
                },
                Draw {
                    rect: [14.0, 39.0, 71.0, 55.0],
                    material: Material::Solid([0.25314078, 0.8680544, 0.6820076, 1.0]),
                    blend: Blend::Src,
                    samples: None,
                },
            ],
        }],
        // The second: multisampled, two ordinary draws.
        vec![Pass {
            extent: (SIDE, SIDE),
            samples: what.others_samples,
            clear: [0.0, 0.0, 0.0, 1.0],
            draws: vec![
                Draw {
                    rect: [9.0, 33.0, 25.0, 71.0],
                    material: Material::Solid([0.29656017, 0.033283897, 0.3366662, 0.8946956]),
                    blend: Blend::Over,
                    samples: None,
                },
                Draw {
                    rect: [22.0, 27.0, 54.0, 56.0],
                    material: Material::Solid([0.70035374, 0.59060365, 0.28776625, 1.0]),
                    blend: Blend::Over,
                    samples: None,
                },
            ],
        }],
        // The third: a small multisampled pass whose draw reaches the equation,
        // composited into a frame of its own. This is the one that carries the
        // advanced blend, and the renderer reads zero of twelve without it.
        vec![
            Pass {
                extent: (31, 25),
                samples: what.others_samples,
                clear: [0.0, 0.0, 0.0, 0.0],
                draws: vec![Draw {
                    rect: [0.0, 0.0, 31.0, 25.0],
                    material: Material::Solid([0.24970609, 0.28284502, 0.2957505, 1.0]),
                    blend: advanced,
                    samples: None,
                }],
            },
            Pass {
                extent: (SIDE, SIDE),
                samples: 1,
                clear: [0.0, 0.0, 0.0, 1.0],
                draws: vec![Draw {
                    rect: [36.0, 17.0, 67.0, 42.0],
                    // The mapping a bounded layer composites through, which is
                    // the recording's own: the target is wider than the layer,
                    // so the columns scale by more than the whole-frame case.
                    material: Material::Image {
                        to_local: [
                            2.064516, 0.0, 0.0, 0.0, 0.0, -2.56, 0.0, 0.0, 0.9032258, 1.88, 1.0,
                            0.0,
                        ],
                        alpha: 0.9840544,
                    },
                    blend: Blend::Over,
                    samples: Some(0),
                }],
            },
        ],
    ]
}

/// A color attachment and the framebuffer that renders into it.
///
/// Created and destroyed per frame, as the renderer's layer targets are:
/// `execute_layers` allocates one per pass and the caller frees them when the
/// frame is done, so a name and its memory come back around every frame.
struct Target {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
}

impl Target {
    /// As `render.rs::create_texture` makes one: sized storage with one level,
    /// linear filtering to match it, and clamped on both axes.
    fn new(gl: &glow::Context, width: i32, height: i32) -> Self {
        // SAFETY: a context is current on this thread, and every object below
        // is created, used and deleted on it.
        unsafe {
            let texture = gl.create_texture().expect("create_texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, width, height);
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
            for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, glow::CLAMP_TO_EDGE as i32);
            }
            let framebuffer = gl.create_framebuffer().expect("create_framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "a target is incomplete"
            );
            Self {
                framebuffer,
                texture,
            }
        }
    }

    fn destroy(self, gl: &glow::Context) {
        // SAFETY: both objects were created by this context and are deleted
        // once.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
    }
}

/// The transient multisample attachment a multisampled pass renders into.
///
/// Allocated per pass and destroyed after the resolve, which is what
/// `render.rs` does: the target stays single-sampled and directly samplable,
/// and the samples live only as long as the pass that wrote them.
struct Multisample {
    framebuffer: glow::Framebuffer,
    renderbuffer: glow::Renderbuffer,
}

impl Multisample {
    fn new(gl: &glow::Context, width: i32, height: i32, samples: i32) -> Self {
        // SAFETY: as `Target::new`.
        unsafe {
            let renderbuffer = gl.create_renderbuffer().expect("create_renderbuffer");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(renderbuffer));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                samples,
                glow::RGBA8,
                width,
                height,
            );
            let framebuffer = gl.create_framebuffer().expect("create_framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(renderbuffer),
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "a multisample attachment is incomplete"
            );
            // The implementation may round the count up and reports no error
            // when it does, so this is read rather than assumed: a pass paying
            // for more samples than it asked for is not the pass being
            // measured.
            let got = gl.get_parameter_i32(glow::SAMPLES);
            assert_eq!(got, samples, "asked for {samples} samples and got {got}");
            Self {
                framebuffer,
                renderbuffer,
            }
        }
    }

    fn destroy(self, gl: &glow::Context) {
        // SAFETY: as `Target::destroy`.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_renderbuffer(self.renderbuffer);
        }
    }
}

/// The renderer's own program, in both of the forms the renderer builds.
struct Program {
    /// For every draw that does not blend under an advanced equation.
    plain: glow::Program,
    /// The same source with the extension and the `blend_support_all_equations`
    /// qualifier inserted, which is the only form the extension lets an
    /// advanced equation apply to. Two programs rather than one because
    /// declaring the qualifier is not free of consequence for a draw that is
    /// not using one -- `context.rs` says this driver renders such a draw
    /// measurably differently.
    blend: glow::Program,
    image_plain: Option<glow::UniformLocation>,
    image_blend: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    vertices: glow::Buffer,
    indices: glow::Buffer,
    paints: glow::Buffer,
    /// What the implementation's uniform-buffer alignment makes of the paint's
    /// size, which is the distance from one draw's block to the next.
    stride: usize,
}

/// The two lines `render::with_advanced_blend` inserts, inserted the same way.
fn with_advanced_blend(source: &str) -> String {
    const VERSION_LINE: &str = "#version 300 es\n";
    assert!(
        source.starts_with(VERSION_LINE),
        "the generated source does not begin with the version line"
    );
    format!(
        "{VERSION_LINE}#extension GL_KHR_blend_equation_advanced : require\n\
         layout(blend_support_all_equations) out;\n{}",
        &source[VERSION_LINE.len()..]
    )
}

fn compile(gl: &glow::Context, vertex: &str, fragment: &str) -> glow::Program {
    // SAFETY: a context is current; the shaders are deleted once attached.
    unsafe {
        let program = gl.create_program().expect("create_program");
        for (stage, source) in [
            (glow::VERTEX_SHADER, vertex),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(stage).expect("create_shader");
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            assert!(
                gl.get_shader_compile_status(shader),
                "compiling {stage:#x}: {}",
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

impl Program {
    /// Both forms of the program, and the buffers a pass fills.
    ///
    /// The vertex stage is the renderer's either way: the lowering is appended
    /// to the fragment, so that is the stage a control on the shader moves.
    fn new(gl: &glow::Context, renderer_shader: bool) -> Self {
        let vertex = emblema_shaders::SOLID_VS_GLSL;
        let fragment = if renderer_shader {
            emblema_shaders::SOLID_FS_GLSL
        } else {
            SMALL_FS
        };
        let plain = compile(gl, vertex, fragment);
        let blend = compile(gl, vertex, &with_advanced_blend(fragment));
        // SAFETY: a context is current, and every name below comes from it.
        unsafe {
            // GLSL ES 3.00 has no `layout(binding = )` for a uniform block, so
            // the binding point is assigned here, as the renderer assigns it.
            for program in [plain, blend] {
                let block = gl
                    .get_uniform_block_index(program, "Paint_block_0Fragment")
                    .expect("the paint block");
                gl.uniform_block_binding(program, block, 0);
            }
            // Naga names a combined sampler after the texture's group and
            // binding rather than after the WGSL variable.
            let image_plain = gl.get_uniform_location(plain, "_group_0_binding_0_fs");
            let image_blend = gl.get_uniform_location(blend, "_group_0_binding_0_fs");

            let vao = gl.create_vertex_array().expect("create_vertex_array");
            let vertices = gl.create_buffer().expect("create_buffer");
            let indices = gl.create_buffer().expect("create_buffer");
            let paints = gl.create_buffer().expect("create_buffer");
            let alignment = gl
                .get_parameter_i32(glow::UNIFORM_BUFFER_OFFSET_ALIGNMENT)
                .max(1) as usize;
            let bytes = MATERIAL_FLOATS * 4;
            Self {
                plain,
                blend,
                image_plain,
                image_blend,
                vao,
                vertices,
                indices,
                paints,
                stride: bytes.div_ceil(alignment) * alignment,
            }
        }
    }
}

/// A vertex as the renderer's own `Vertex` is laid out: a homogeneous position,
/// a texture coordinate, and a premultiplied color.
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

/// The four corners of a rectangle, in the clip space the shader reads.
///
/// Pixels in, clip out: `2 * p / side - 1` on each axis, with the third
/// component one. Nothing turns y over here -- the vertex stage does it, which
/// is why a readback needs no row flip either.
fn quad(rect: [f32; 4], extent: (i32, i32)) -> [Vertex; 4] {
    let (w, h) = (extent.0 as f32, extent.1 as f32);
    let clip = |x: f32, y: f32| [2.0 * x / w - 1.0, 2.0 * y / h - 1.0, 1.0];
    let vertex = |x: f32, y: f32| Vertex {
        position: clip(x, y),
        uv: [0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    };
    [
        vertex(rect[0], rect[1]),
        vertex(rect[2], rect[1]),
        vertex(rect[2], rect[3]),
        vertex(rect[0], rect[3]),
    ]
}

fn cast_bytes<T>(values: &[T]) -> &[u8] {
    // SAFETY: every element is plain data, and the slice's length in bytes is
    // its length times the element's size.
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

/// Set the blend state a draw asks for, as `render.rs::apply_blend` sets it.
fn apply_blend(gl: &glow::Context, blend: Blend) {
    // SAFETY: a context is current.
    unsafe {
        match blend {
            Blend::Src => gl.disable(glow::BLEND),
            Blend::Over => {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                );
                gl.blend_equation(glow::FUNC_ADD);
            }
            Blend::Advanced(equation) => {
                gl.enable(glow::BLEND);
                // Ignored by an advanced equation but not unset: stale factors
                // would take effect the moment a later draw switched back to
                // `FUNC_ADD` without setting its own.
                gl.blend_func_separate(glow::ONE, glow::ZERO, glow::ONE, glow::ZERO);
                gl.blend_equation(equation);
            }
        }
    }
}

/// Render one frame's passes into fresh targets, and return the last one's
/// bytes.
fn render(
    gl: &glow::Context,
    program: &Program,
    placeholder: glow::Texture,
    blend_barrier: Option<extern "C" fn()>,
    passes: &[Pass],
) -> Vec<u8> {
    let mut targets: Vec<Target> = Vec::with_capacity(passes.len());
    // SAFETY: a context is current for every call below, and every object is
    // created here and deleted before returning.
    unsafe {
        for pass in passes {
            let target = Target::new(gl, pass.extent.0, pass.extent.1);
            let multisample = (pass.samples > 1)
                .then(|| Multisample::new(gl, pass.extent.0, pass.extent.1, pass.samples));
            let framebuffer = multisample
                .as_ref()
                .map_or(target.framebuffer, |ms| ms.framebuffer);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.viewport(0, 0, pass.extent.0, pass.extent.1);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            gl.color_mask(true, true, true, true);
            gl.clear_color(pass.clear[0], pass.clear[1], pass.clear[2], pass.clear[3]);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(program.plain));
            gl.bind_vertex_array(Some(program.vao));

            // One buffer for the pass, as a batch is: every draw's geometry
            // uploaded once and indexed into.
            let mut vertices: Vec<Vertex> = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            for draw in &pass.draws {
                let base = vertices.len() as u32;
                vertices.extend_from_slice(&quad(draw.rect, pass.extent));
                indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(program.vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, cast_bytes(&vertices), glow::STREAM_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(program.indices));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                cast_bytes(&indices),
                glow::STREAM_DRAW,
            );

            // Written into the padded layout directly, so the gaps the
            // implementation's alignment requires are zeros rather than
            // whatever the last pass left there.
            let mut paints = vec![0u8; pass.draws.len().max(1) * program.stride];
            for (index, draw) in pass.draws.iter().enumerate() {
                let packed = draw.material.pack();
                let at = index * program.stride;
                paints[at..at + MATERIAL_FLOATS * 4].copy_from_slice(cast_bytes(&packed));
            }
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(program.paints));
            gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, &paints, glow::STREAM_DRAW);

            let stride = std::mem::size_of::<Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, stride, 20);

            for (index, draw) in pass.draws.iter().enumerate() {
                apply_blend(gl, draw.blend);
                // Before every advanced draw, not only where two of them
                // overlap: an advanced equation reads the destination, and the
                // pass's own clear is a write.
                if let (Some(barrier), Blend::Advanced(_)) = (blend_barrier, draw.blend) {
                    barrier();
                }
                // Every unit the layout has, as the renderer binds them: a unit
                // left pointing at a previous frame's texture is a texture that
                // may since have been deleted.
                for unit in 0..UNITS {
                    let texture = match (unit, draw.samples) {
                        (0, Some(source)) => targets[source].texture,
                        _ => placeholder,
                    };
                    gl.active_texture(glow::TEXTURE0 + unit);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                }
                gl.active_texture(glow::TEXTURE0);

                let (object, image) = match draw.blend {
                    Blend::Advanced(_) => (program.blend, program.image_blend.as_ref()),
                    _ => (program.plain, program.image_plain.as_ref()),
                };
                gl.use_program(Some(object));
                if let Some(location) = image {
                    gl.uniform_1_i32(Some(location), 0);
                }
                // One block for the whole pass, rebound to this draw's range.
                // The size is the material rather than the stride: a range
                // reaching past the last material would run off the buffer.
                gl.bind_buffer_range(
                    glow::UNIFORM_BUFFER,
                    0,
                    Some(program.paints),
                    (index * program.stride) as i32,
                    (MATERIAL_FLOATS * 4) as i32,
                );
                gl.draw_elements(
                    glow::TRIANGLES,
                    6,
                    glow::UNSIGNED_INT,
                    (index * 6 * 4) as i32,
                );
            }

            gl.disable(glow::BLEND);
            gl.bind_vertex_array(None);
            if let Some(ms) = multisample {
                // NEAREST, not LINEAR: a blit whose read buffer is multisampled
                // and whose draw buffer is not must use NEAREST, and the resolve
                // itself is what averages the samples.
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(ms.framebuffer));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target.framebuffer));
                gl.blit_framebuffer(
                    0,
                    0,
                    pass.extent.0,
                    pass.extent.1,
                    0,
                    0,
                    pass.extent.0,
                    pass.extent.1,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
                ms.destroy(gl);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            targets.push(target);
        }

        // GLES has no validation layer, so this is the whole of what can be
        // asked cheaply: a driver that minded any of the above would have set
        // an error. It is not proof of valid usage and nothing here treats it
        // as such.
        let error = gl.get_error();
        assert_eq!(
            error,
            glow::NO_ERROR,
            "the driver reported {error:#x} for this sequence, so it is not a finding"
        );

        let last = passes.last().expect("a frame has at least one pass");
        let mut pixels = vec![0u8; (last.extent.0 * last.extent.1 * 4) as usize];
        let target = targets.last().expect("a target per pass");
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.framebuffer));
        gl.read_pixels(
            0,
            0,
            last.extent.0,
            last.extent.1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut pixels),
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);

        // Freed here, which is the point of allocating them per frame: the
        // renderer's layer targets live exactly one frame, so their names and
        // their memory come back around.
        for target in targets {
            target.destroy(gl);
        }
        pixels
    }
}

/// A one-by-one white texture, for the units a draw does not sample.
fn placeholder(gl: &glow::Context) -> glow::Texture {
    // SAFETY: a context is current, and the source is sized for the one pixel
    // being written.
    unsafe {
        let texture = gl.create_texture().expect("create_texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            Some(&[255u8, 255, 255, 255]),
        );
        // A texture with the default mipmap filter and no mipmaps is incomplete
        // and samples as black, which would make the placeholder do exactly
        // what it exists to avoid.
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
        texture
    }
}

/// Print where two renders of the subject differ, and how.
fn report(name: &str, first: &[u8], again: &[u8], detail: bool) -> i32 {
    let worst = first
        .iter()
        .zip(again.iter())
        .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
        .max()
        .unwrap_or(0);
    let differing = first
        .iter()
        .zip(again.iter())
        .filter(|(a, b)| a != b)
        .count();
    let rgba = |pixels: &[u8], x: usize, y: usize| {
        let at = ((y * SIDE as usize) + x) * 4;
        format!(
            "[{:3},{:3},{:3},{:3}]",
            pixels[at],
            pixels[at + 1],
            pixels[at + 2],
            pixels[at + 3]
        )
    };
    println!(
        "{name:24} worst {worst:3} level(s), {differing:5} byte(s) of {} differ",
        first.len()
    );

    // `AT=x,y` prints that pixel from both renders whether or not they differ,
    // which is what a clean driver has to be asked for: its answer is the one
    // the two are measured against, and it has no difference to report.
    if let Some((x, y)) = std::env::var("AT").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some((
            a.trim().parse::<usize>().ok()?,
            b.trim().parse::<usize>().ok()?,
        ))
    }) {
        println!(
            "  at ({x},{y}): first {} again {}",
            rgba(first, x, y),
            rgba(again, x, y)
        );
    }
    if worst > 0 && detail {
        // Which way, and in which channel. A one-sided error is a different
        // claim from a wobble: dither or rounding scatters both ways, and a
        // contribution that is lost or doubled does not.
        let (mut lower, mut higher) = (0usize, 0usize);
        let mut channels = [0usize; 4];
        for (i, (a, b)) in first.iter().zip(again.iter()).enumerate() {
            match b.cmp(a) {
                std::cmp::Ordering::Less => {
                    lower += 1;
                    channels[i % 4] += 1;
                }
                std::cmp::Ordering::Greater => {
                    higher += 1;
                    channels[i % 4] += 1;
                }
                std::cmp::Ordering::Equal => {}
            }
        }
        println!(
            "  the second render is lower in {lower} and higher in {higher}; \
             by channel r{} g{} b{} a{}",
            channels[0], channels[1], channels[2], channels[3]
        );
        if let Some(at) = first
            .iter()
            .zip(again.iter())
            .position(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() == worst)
        {
            let (x, y) = ((at / 4) % SIDE as usize, (at / 4) / SIDE as usize);
            println!(
                "  worst at ({x},{y}): first {} again {}",
                rgba(first, x, y),
                rgba(again, x, y)
            );
        }
        if std::env::var_os("MAP").is_some() {
            for y in 0..SIDE as usize {
                let mut row = String::new();
                let mut any = false;
                for x in 0..SIDE as usize {
                    let at = ((y * SIDE as usize) + x) * 4;
                    let d = (0..4)
                        .map(|c| (i32::from(first[at + c]) - i32::from(again[at + c])).abs())
                        .max()
                        .unwrap_or(0);
                    any |= d > 0;
                    row.push(match d {
                        0 => '.',
                        1 => '-',
                        2..=4 => '+',
                        _ => '#',
                    });
                }
                if any {
                    println!("  y={y:3} {row}");
                }
            }
        }
    }
    worst
}

fn main() {
    let wanted = std::env::args().nth(1);
    // The fragment program as the driver gets it, qualifier and all. What a
    // report upstream carries is this text and a draw that miscompiles it, and
    // printing it is how it leaves here without a Rust toolchain at the other
    // end.
    if wanted.as_deref() == Some("--shader") {
        print!("{}", with_advanced_blend(emblema_shaders::SOLID_FS_GLSL));
        return;
    }
    if wanted.as_deref() == Some("--list") {
        println!("variants:");
        for (name, _, what) in VARIANTS {
            println!("  {name:24} {what}");
        }
        return;
    }

    // The loop is intermittent -- the renderer reads ten hits of twelve -- so
    // one pass through it cannot tell a clean arrangement from a miss. `RUNS`
    // is how many times to ask, and a control has to read zero over all of
    // them to mean anything.
    let runs: usize = std::env::var("RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);

    let mut announced = false;
    for (name, what, _) in VARIANTS {
        if let Some(only) = wanted.as_deref() {
            if only != *name {
                continue;
            }
        }
        let subject = subject(*what);
        let others = others(*what);
        let mut hits = 0;
        for _ in 0..runs {
            // A context per run, as `ablate-the-frame-loop` takes one per
            // variant: what is being measured is what one frame leaves for the
            // next, so a shared context would carry the run before it as much
            // as the run. The programs are compiled per context for the same
            // reason, which is also what the renderer does.
            let opened = if announced {
                bare::Bare::open_silent()
            } else {
                bare::Bare::open()
            };
            let Some(bare) = opened else {
                return;
            };
            if !announced {
                announced = true;
                println!();
            }
            let gl = &bare.gl;
            let program = Program::new(gl, what.renderer_shader);
            let placeholder = placeholder(gl);

            let first = render(gl, &program, placeholder, bare.blend_barrier, &subject);
            for frame in &others {
                let _ = render(gl, &program, placeholder, bare.blend_barrier, frame);
            }
            let again = render(gl, &program, placeholder, bare.blend_barrier, &subject);
            if report(name, &first, &again, runs == 1) > 0 {
                hits += 1;
            }
        }
        if runs > 1 {
            println!("{name:24} {hits} hit(s) of {runs}");
        }
    }
}
