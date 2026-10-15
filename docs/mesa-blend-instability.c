// SPDX-License-Identifier: Apache-2.0
//
// A GLES frame that radeonsi renders differently the second time.
//
// This is the artifact for a Mesa bug report, and it is self-contained: nothing
// but EGL and GLES, no build system, no library beyond libEGL and libGLESv2, no
// generated code, and no part of the project it came out of. The tree it sits
// in never compiles it -- that tree is pure Rust and says so -- and this file is
// here beside the document that explains the defect.
//
//   cc -O2 -o blend-instability docs/mesa-blend-instability.c -lEGL -lGLESv2
//   ./blend-instability                          # twelve runs
//   ./blend-instability --generated a.vert a.frag  # the renderer's own shaders
//
// RUNS=n sets how many times to ask, TAPS=n the blur's half-width, ROUNDS=n how
// many times the disturbing frames run, EQUATION=<hex> the composite's
// equation, and AT=x,y prints that pixel from both renderings.
//
// # What it does
//
// Four render passes over a 128 by 128 frame, all single-sample:
//
//   0  two solid draws over a transparent clear          -- a layer's content
//   1  the first half of a separable blur, sampling 0    -- Src
//   2  the second half, sampling 1                       -- Src
//   3  a solid draw, then pass 2 drawn as a texture under GL_OVERLAY_KHR
//
// then three other frames, two of them four-sample, one of which draws under
// GL_DIFFERENCE_KHR, then the four passes again -- and the second rendering of
// the frame differs from the first.
//
// The two shaders below are fifty lines together. They are not the renderer's,
// and they draw the renderer's frame to the byte: with `TAPS=5` the picture
// hash this prints is the same one the generated 964-line fragment program
// produces, which `--generated` runs for comparison.
//
// # What it reads here
//
// Mesa 26.2.4, radeonsi on a Raphael/Mendocino iGPU, twelve runs of twelve:
//
//   worst 1 level, 15 of 65536 bytes differ, every one lower, every one green
//   at the worst pixel: first [136, 87, 44] then [136, 86, 44]
//
// llvmpipe reads [136, 87, 44] both times (AT=6,85 asks either driver), so the
// first rendering is the exact one and the re-rendering is wrong. Every control
// reads 0 of 12:
//
//   radeonsi_inline_uniforms=false ./blend-instability     uniform inlining off
//   AMD_DEBUG=nooptvariant ./blend-instability             no optimized variant
//   EQUATION=0x929E ./blend-instability                    GL_DIFFERENCE_KHR
//   LIBGL_ALWAYS_SOFTWARE=1 ./blend-instability            llvmpipe
//
// # Two things this program cannot see
//
// **The eight-bit floor.** The error is one level, so a frame whose arithmetic
// puts it below half a quantum reads as zero here with the miscompile still in
// place. `TAPS` is where that shows: eleven to seventeen iterations of the blur
// loop report it and nothing outside that range does, while `AMD_DEBUG=ps,nir`
// shows the same specializations compiled either way --
// `opt.inline_uniforms = 1 (0x3, 0x0, 0x0, 0x0)` among them -- and the renderer
// this was reduced from reproduces at every blur deviation from 2.5 to 5.5. So
// a zero from this program means "not visible in this frame", not "not
// miscompiled", and that holds for every control above as much as for TAPS.
//
// **The dose.** ROUNDS is how many times the three disturbing frames run
// between the two renderings. One is the default and is what the renderer does,
// and with the shaders above that is already twelve of twelve. Under
// `--generated` it is not, and the rate follows the dose:
//
//   ROUNDS=1   2 to 8 of 12
//   ROUNDS=2   9 of 12
//   ROUNDS=4   8 to 12 of 12
//   ROUNDS=8   12 of 12
//
// so whatever those frames leave behind accumulates rather than arriving all at
// once -- and how much of it a frame needs before the error clears the floor
// depends on the frame, which is the paragraph above again.

#include <EGL/egl.h>
#include <GLES3/gl3.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// The extension's enumerants, spelled out rather than taken from a header: the
// two headers that carry them disagree about which one does.
#define GL_OVERLAY_KHR 0x9296
#define GL_HARDLIGHT_KHR 0x929B
#define GL_DIFFERENCE_KHR 0x929E
#define EGL_PLATFORM_SURFACELESS_MESA 0x31DD

#define SIDE 128

// The paint block the shader declares: sixteen four-component vectors, and
// where each thing the frame needs sits in it.
#define MATERIAL_FLOATS 64
#define STOPS 0
#define GEOMETRY 20
#define TO_LOCAL 24
#define PARAMS 36
#define FILTER_PARAMS 60

// The material kinds the shader dispatches on, of which this frame uses three.
#define KIND_SOLID 0.0f
#define KIND_IMAGE 4.0f
#define KIND_BLUR 6.0f

// The tint blend every draw carries, and the identity against the opaque white
// the vertices hold.
#define TINT_MODULATE 13.0f

// Clip space to the whole of the sampled texture: three columns of a
// three-by-three in column order, each padded to four, with y turned over
// because the vertex stage negates it on the way out.
static const float WHOLE[12] = {0.5f, 0.0f, 0.0f,  0.0f, 0.0f, -0.5f,
                                0.0f, 0.0f, 0.5f,  0.5f, 1.0f, 0.0f};

// The vertex stage: a position, a texture coordinate and a color per vertex,
// the three varyings the fragment stage reads -- named as the generated program
// names them, because ES 3.00 matches a varying across stages by name -- and the
// clip-space convention
// the renderer's own generated vertex program uses -- y negated and z mapped
// from GL's range to Vulkan's, which is what makes the two interchangeable
// here.
static const char *const VERTEX = "#version 300 es\n"
                                  "precision highp float;\n"
                                  "layout(location = 0) in vec3 in_position;\n"
                                  "layout(location = 1) in vec2 in_uv;\n"
                                  "layout(location = 2) in vec4 in_color;\n"
                                  "smooth out vec3 _vs2fs_location0;\n"
                                  "smooth out vec2 _vs2fs_location1;\n"
                                  "smooth out vec4 _vs2fs_location2;\n"
                                  "void main() {\n"
                                  "    gl_Position = vec4(in_position.xy, 0.0, in_position.z);\n"
                                  "    _vs2fs_location0 = in_position;\n"
                                  "    _vs2fs_location1 = in_uv;\n"
                                  "    _vs2fs_location2 = in_color;\n"
                                  "    gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);\n"
                                  "}\n";

// The fragment stage: the three materials this frame draws, over the paint
// block the C below fills in. `%d` is the blur's half-width, which `TAPS` sets
// -- it is a literal in the shader rather than a uniform because the point of
// the knob is to change what the compiler sees.
//
// The block, its instance name, the varyings and the sampler are named as the
// renderer's generated program names them, so `--generated` can swap one
// program for the other with nothing else moving.
static const char *const FRAGMENT =
    "#version 300 es\n"
    "precision highp float;\n"
    "precision highp int;\n"
    "struct Paint {\n"
    "    vec4 stops[4];\n"
    "    vec4 offsets;\n"
    "    vec4 geometry;\n"
    "    vec4 to_local[3];\n"
    "    vec4 params;\n"
    "    vec4 recolor[4];\n"
    "    vec4 filter_offset;\n"
    "    vec4 filter_params;\n"
    "};\n"
    "layout(std140) uniform Paint_block_0Fragment { Paint _group_1_binding_0_fs; };\n"
    "uniform highp sampler2D _group_0_binding_0_fs;\n"
    "smooth in vec3 _vs2fs_location0;\n"
    "smooth in vec2 _vs2fs_location1;\n"
    "smooth in vec4 _vs2fs_location2;\n"
    "layout(location = 0) out vec4 _fs2p_location0;\n"
    "vec2 to_local_space(vec3 clip) {\n"
    "    Paint p = _group_1_binding_0_fs;\n"
    "    vec3 mapped = p.to_local[0].xyz * clip.x\n"
    "                + p.to_local[1].xyz * clip.y\n"
    "                + p.to_local[2].xyz * clip.z;\n"
    "    return mapped.xy / max(mapped.z, 1e-6);\n"
    "}\n"
    "void main() {\n"
    "    Paint p = _group_1_binding_0_fs;\n"
    "    float kind = p.params.y;\n"
    "    vec4 color;\n"
    "    if (kind < 0.5) {\n"
    "        vec4 c = p.stops[0];\n"
    "        color = vec4(c.rgb * c.a, c.a);\n"
    "    } else if (kind > 5.5 && kind < 6.5) {\n"
    "        vec2 uv = to_local_space(_vs2fs_location0);\n"
    "        vec2 step = p.geometry.zw;\n"
    "        float sigma = max(p.params.z, 1e-6);\n"
    "        vec4 sum = vec4(0.0);\n"
    "        float total = 0.0;\n"
    "        for (int i = -%d; i <= %d; i++) {\n"
    "            float w = exp(-0.5 * float(i * i) / (sigma * sigma));\n"
    "            sum += texture(_group_0_binding_0_fs, uv + step * float(i)) * w;\n"
    "            total += w;\n"
    "        }\n"
    "        color = sum / total;\n"
    "    } else {\n"
    "        vec2 uv = clamp(to_local_space(_vs2fs_location0), 0.0, 1.0);\n"
    "        color = texture(_group_0_binding_0_fs, uv) * p.geometry.z;\n"
    "    }\n"
    "    _fs2p_location0 = _vs2fs_location2 * color;\n"
    "}\n";

enum blend { BLEND_SRC, BLEND_OVER, BLEND_ADVANCED };

struct draw {
    // In pixels of the pass's own target.
    float rect[4];
    float kind;
    // Solid: the color, straight rather than premultiplied. Image: the alpha in
    // `alpha`. Blur: the step and the deviation.
    float color[4];
    const float *to_local;
    float step[2];
    float sigma;
    float alpha;
    enum blend blend;
    GLenum equation;
    // Which earlier pass of this frame the draw samples, or -1.
    int samples;
};

struct pass {
    int width;
    int height;
    int samples;
    float clear[4];
    int count;
    struct draw draws[2];
};

struct frame {
    int count;
    struct pass passes[4];
};

// A vertex as the renderer lays one out: a homogeneous position, a texture
// coordinate, and a premultiplied color.
struct vertex {
    float position[3];
    float uv[2];
    float color[4];
};

static void *read_file(const char *path, size_t *length) {
    FILE *file = fopen(path, "rb");
    if (!file) {
        fprintf(stderr,
                "cannot open %s.\n"
                "The two shader files come out of the probe this reproduces:\n"
                "  probe-the-layer-composite -- --vertex > solid.vert\n"
                "  probe-the-layer-composite -- --shader > solid.frag\n",
                path);
        exit(1);
    }
    fseek(file, 0, SEEK_END);
    long size = ftell(file);
    fseek(file, 0, SEEK_SET);
    char *text = malloc((size_t)size + 1);
    if (!text || fread(text, 1, (size_t)size, file) != (size_t)size) {
        fprintf(stderr, "cannot read %s\n", path);
        exit(1);
    }
    text[size] = '\0';
    fclose(file);
    if (length) {
        *length = (size_t)size;
    }
    return text;
}

// The two lines the renderer inserts after the version line, inserted the same
// way. GL_KHR_blend_equation_advanced will not apply one of its equations to a
// program that does not declare this, and declaring it is not free of
// consequence for a draw that is not using one -- which is why there are two
// programs below and not one.
static char *qualified(const char *source) {
    static const char version[] = "#version 300 es\n";
    const size_t length = sizeof version - 1;
    if (strncmp(source, version, length) != 0) {
        fprintf(stderr, "a fragment source does not begin with the version line\n");
        exit(1);
    }
    static const char lines[] =
        "#extension GL_KHR_blend_equation_advanced : require\n"
        "layout(blend_support_all_equations) out;\n";
    char *out = malloc(strlen(source) + sizeof lines);
    if (!out) {
        exit(1);
    }
    memcpy(out, version, length);
    memcpy(out + length, lines, sizeof lines - 1);
    strcpy(out + length + sizeof lines - 1, source + length);
    return out;
}

static GLuint compile(GLenum stage, const char *source) {
    GLuint shader = glCreateShader(stage);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint ok = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[4096];
        glGetShaderInfoLog(shader, sizeof log, NULL, log);
        fprintf(stderr, "compiling %#x: %s\n", stage, log);
        exit(1);
    }
    return shader;
}

static GLuint link_program(const char *vertex, const char *fragment) {
    GLuint program = glCreateProgram();
    GLuint stages[2] = {compile(GL_VERTEX_SHADER, vertex),
                        compile(GL_FRAGMENT_SHADER, fragment)};
    for (int i = 0; i < 2; i++) {
        glAttachShader(program, stages[i]);
        glDeleteShader(stages[i]);
    }
    glLinkProgram(program);
    GLint ok = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &ok);
    if (!ok) {
        char log[4096];
        glGetProgramInfoLog(program, sizeof log, NULL, log);
        fprintf(stderr, "linking: %s\n", log);
        exit(1);
    }
    // GLSL ES 3.00 has no layout(binding = ) for a uniform block, so the
    // binding point is assigned here.
    GLuint block = glGetUniformBlockIndex(program, "Paint_block_0Fragment");
    if (block == GL_INVALID_INDEX) {
        fprintf(stderr, "the fragment program has no Paint_block_0Fragment\n");
        exit(1);
    }
    glUniformBlockBinding(program, block, 0);
    return program;
}

// The one identifier in this file that is not GL's or C's: the generated
// sampler is named after the texture's group and binding rather than after the
// variable it came from.
static GLint sampler_of(GLuint program) {
    return glGetUniformLocation(program, "_group_0_binding_0_fs");
}

static void pack(const struct draw *draw, float *out) {
    memset(out, 0, MATERIAL_FLOATS * sizeof *out);
    // Every draw carries these two: one stop in use, and the tint blend the
    // vertex color goes through.
    out[PARAMS] = 1.0f;
    out[FILTER_PARAMS + 1] = TINT_MODULATE;
    out[PARAMS + 1] = draw->kind;
    if (draw->kind == KIND_SOLID) {
        memcpy(out + STOPS, draw->color, sizeof draw->color);
        return;
    }
    memcpy(out + TO_LOCAL, draw->to_local, 12 * sizeof *out);
    if (draw->kind == KIND_BLUR) {
        out[GEOMETRY + 2] = draw->step[0];
        out[GEOMETRY + 3] = draw->step[1];
        out[PARAMS + 2] = draw->sigma;
        return;
    }
    // An image: the source rectangle, which is the whole texture, and the tint,
    // which is white and opaque and therefore the identity. The tile mode is
    // clamp and the sampling is linear, both of which are zero.
    out[STOPS + 2] = 1.0f;
    out[STOPS + 3] = 1.0f;
    out[STOPS + 4] = 1.0f;
    out[STOPS + 5] = 1.0f;
    out[STOPS + 6] = 1.0f;
    out[STOPS + 7] = 1.0f;
    out[GEOMETRY + 2] = draw->alpha;
}

struct program {
    GLuint plain;
    GLuint blend;
    GLint image_plain;
    GLint image_blend;
    GLuint vao;
    GLuint vertices;
    GLuint indices;
    GLuint paints;
    GLsizei stride;
};

static void apply_blend(const struct draw *draw) {
    switch (draw->blend) {
    case BLEND_SRC:
        // Writing the source outright needs no blend unit.
        glDisable(GL_BLEND);
        break;
    case BLEND_OVER:
        glEnable(GL_BLEND);
        glBlendFuncSeparate(GL_ONE, GL_ONE_MINUS_SRC_ALPHA, GL_ONE,
                            GL_ONE_MINUS_SRC_ALPHA);
        glBlendEquation(GL_FUNC_ADD);
        break;
    case BLEND_ADVANCED:
        glEnable(GL_BLEND);
        // Ignored by an advanced equation but not left stale: they would take
        // effect the moment a later draw switched back to GL_FUNC_ADD without
        // setting its own.
        glBlendFuncSeparate(GL_ONE, GL_ZERO, GL_ONE, GL_ZERO);
        glBlendEquation(draw->equation);
        break;
    }
}

typedef void (*barrier_fn)(void);

// Render one frame's passes into fresh targets and read the last one back. The
// targets are allocated per frame and freed at the end of it, which is what the
// renderer does with a layer's target.
static void render(const struct program *program, GLuint placeholder,
                   barrier_fn barrier, const struct frame *frame,
                   unsigned char *pixels) {
    GLuint textures[4] = {0};
    GLuint framebuffers[4] = {0};

    for (int index = 0; index < frame->count; index++) {
        const struct pass *pass = &frame->passes[index];
        glGenTextures(1, &textures[index]);
        glBindTexture(GL_TEXTURE_2D, textures[index]);
        glTexStorage2D(GL_TEXTURE_2D, 1, GL_RGBA8, pass->width, pass->height);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glGenFramebuffers(1, &framebuffers[index]);
        glBindFramebuffer(GL_FRAMEBUFFER, framebuffers[index]);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0,
                               GL_TEXTURE_2D, textures[index], 0);
        if (glCheckFramebufferStatus(GL_FRAMEBUFFER) !=
            GL_FRAMEBUFFER_COMPLETE) {
            fprintf(stderr, "a target is incomplete\n");
            exit(1);
        }

        // A multisampled pass renders into a transient renderbuffer and is
        // resolved into the texture, so the texture stays samplable.
        GLuint ms_framebuffer = 0, ms_renderbuffer = 0;
        if (pass->samples > 1) {
            glGenRenderbuffers(1, &ms_renderbuffer);
            glBindRenderbuffer(GL_RENDERBUFFER, ms_renderbuffer);
            glRenderbufferStorageMultisample(GL_RENDERBUFFER, pass->samples,
                                             GL_RGBA8, pass->width,
                                             pass->height);
            glGenFramebuffers(1, &ms_framebuffer);
            glBindFramebuffer(GL_FRAMEBUFFER, ms_framebuffer);
            glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0,
                                      GL_RENDERBUFFER, ms_renderbuffer);
            // The implementation may round the count up and reports no error
            // when it does, so this is read rather than assumed: a pass paying
            // for more samples than it asked for is not the pass being
            // measured.
            if (glCheckFramebufferStatus(GL_FRAMEBUFFER) !=
                GL_FRAMEBUFFER_COMPLETE) {
                fprintf(stderr, "a multisample attachment is incomplete\n");
                exit(1);
            }
            GLint got = 0;
            glGetIntegerv(GL_SAMPLES, &got);
            if (got != pass->samples) {
                fprintf(stderr, "asked for %d samples and got %d\n",
                        pass->samples, got);
                exit(1);
            }
        }

        glViewport(0, 0, pass->width, pass->height);
        glDisable(GL_SCISSOR_TEST);
        glDisable(GL_DEPTH_TEST);
        glDisable(GL_CULL_FACE);
        glDisable(GL_STENCIL_TEST);
        glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
        glClearColor(pass->clear[0], pass->clear[1], pass->clear[2],
                     pass->clear[3]);
        glClear(GL_COLOR_BUFFER_BIT);

        // Bound before the pass's buffers are filled, as the renderer binds it,
        // and switched per draw below.
        glUseProgram(program->plain);
        glBindVertexArray(program->vao);

        struct vertex vertices[8];
        unsigned indices[12];
        for (int i = 0; i < pass->count; i++) {
            const float *rect = pass->draws[i].rect;
            float w = (float)pass->width, h = (float)pass->height;
            const float xs[4] = {rect[0], rect[2], rect[2], rect[0]};
            const float ys[4] = {rect[1], rect[1], rect[3], rect[3]};
            for (int corner = 0; corner < 4; corner++) {
                struct vertex *v = &vertices[i * 4 + corner];
                // Pixels to clip space. Nothing turns y over here: the vertex
                // stage does it, which is why a readback needs no row flip.
                v->position[0] = 2.0f * xs[corner] / w - 1.0f;
                v->position[1] = 2.0f * ys[corner] / h - 1.0f;
                v->position[2] = 1.0f;
                v->uv[0] = v->uv[1] = 0.0f;
                v->color[0] = v->color[1] = v->color[2] = v->color[3] = 1.0f;
            }
            unsigned base = (unsigned)(i * 4);
            unsigned quad[6] = {base, base + 1, base + 2,
                                base, base + 2, base + 3};
            memcpy(indices + i * 6, quad, sizeof quad);
        }
        glBindBuffer(GL_ARRAY_BUFFER, program->vertices);
        glBufferData(GL_ARRAY_BUFFER,
                     (GLsizeiptr)(pass->count * 4 * sizeof *vertices), vertices,
                     GL_STREAM_DRAW);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, program->indices);
        glBufferData(GL_ELEMENT_ARRAY_BUFFER,
                     (GLsizeiptr)(pass->count * 6 * sizeof *indices), indices,
                     GL_STREAM_DRAW);

        // One block per draw, written into the padded layout directly so the
        // gaps the implementation's alignment requires are zeros. Sized from
        // that alignment rather than from a guess at it: it is 256 bytes here
        // and the specification sets no ceiling.
        size_t span = (size_t)pass->count * (size_t)program->stride;
        unsigned char *paints = calloc(span, 1);
        if (!paints) {
            exit(1);
        }
        for (int i = 0; i < pass->count; i++) {
            float packed[MATERIAL_FLOATS];
            pack(&pass->draws[i], packed);
            memcpy(paints + (size_t)i * (size_t)program->stride, packed,
                   sizeof packed);
        }
        glBindBuffer(GL_UNIFORM_BUFFER, program->paints);
        glBufferData(GL_UNIFORM_BUFFER, (GLsizeiptr)span, paints,
                     GL_STREAM_DRAW);
        free(paints);

        GLsizei stride = (GLsizei)sizeof(struct vertex);
        glEnableVertexAttribArray(0);
        glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, stride, (void *)0);
        glEnableVertexAttribArray(1);
        glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, stride, (void *)12);
        glEnableVertexAttribArray(2);
        glVertexAttribPointer(2, 4, GL_FLOAT, GL_FALSE, stride, (void *)20);

        for (int i = 0; i < pass->count; i++) {
            const struct draw *draw = &pass->draws[i];
            apply_blend(draw);
            // Before every advanced draw, not only where two of them overlap:
            // an advanced equation reads the destination, and the pass's own
            // clear is a write.
            if (barrier && draw->blend == BLEND_ADVANCED) {
                barrier();
            }
            // Every unit the layout has, as the renderer binds them.
            for (int unit = 0; unit < 4; unit++) {
                GLuint texture = placeholder;
                if (unit == 0 && draw->samples >= 0) {
                    texture = textures[draw->samples];
                }
                glActiveTexture((GLenum)(GL_TEXTURE0 + unit));
                glBindTexture(GL_TEXTURE_2D, texture);
            }
            glActiveTexture(GL_TEXTURE0);
            GLuint object = draw->blend == BLEND_ADVANCED ? program->blend
                                                          : program->plain;
            GLint image = draw->blend == BLEND_ADVANCED ? program->image_blend
                                                        : program->image_plain;
            glUseProgram(object);
            if (image >= 0) {
                glUniform1i(image, 0);
            }
            // The size is the material rather than the stride: a range
            // reaching past the last material would run off the buffer.
            glBindBufferRange(GL_UNIFORM_BUFFER, 0, program->paints,
                              (GLintptr)(i * program->stride),
                              MATERIAL_FLOATS * (GLsizeiptr)sizeof(float));
            glDrawElements(GL_TRIANGLES, 6, GL_UNSIGNED_INT,
                           (void *)(size_t)(i * 6 * 4));
        }

        glDisable(GL_BLEND);
        glBindVertexArray(0);
        if (pass->samples > 1) {
            // NEAREST, not LINEAR: a blit whose read buffer is multisampled and
            // whose draw buffer is not must use NEAREST, and the resolve itself
            // is what averages the samples.
            glBindFramebuffer(GL_READ_FRAMEBUFFER, ms_framebuffer);
            glBindFramebuffer(GL_DRAW_FRAMEBUFFER, framebuffers[index]);
            glBlitFramebuffer(0, 0, pass->width, pass->height, 0, 0, pass->width,
                              pass->height, GL_COLOR_BUFFER_BIT, GL_NEAREST);
            glBindFramebuffer(GL_READ_FRAMEBUFFER, 0);
            glBindFramebuffer(GL_DRAW_FRAMEBUFFER, 0);
            glDeleteFramebuffers(1, &ms_framebuffer);
            glDeleteRenderbuffers(1, &ms_renderbuffer);
        }
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
    }

    // GLES has no validation layer, so this is the whole of what can be asked
    // cheaply: a driver that minded any of the above would have set an error.
    GLenum error = glGetError();
    if (error != GL_NO_ERROR) {
        fprintf(stderr, "the driver reported %#x, so this is not a finding\n",
                error);
        exit(1);
    }

    const struct pass *last = &frame->passes[frame->count - 1];
    glBindFramebuffer(GL_READ_FRAMEBUFFER, framebuffers[frame->count - 1]);
    glReadPixels(0, 0, last->width, last->height, GL_RGBA, GL_UNSIGNED_BYTE,
                 pixels);
    glBindFramebuffer(GL_READ_FRAMEBUFFER, 0);

    for (int index = 0; index < frame->count; index++) {
        glDeleteFramebuffers(1, &framebuffers[index]);
        glDeleteTextures(1, &textures[index]);
    }
}

// The four passes, with every rectangle, color, alpha and deviation the
// renderer's own recording of this frame was measured to carry.
static struct frame subject(GLenum equation) {
    const float step = 1.0f / (float)SIDE;
    const float whole[4] = {0.0f, 0.0f, (float)SIDE, (float)SIDE};
    struct frame frame = {0};
    frame.count = 4;

    struct pass *content = &frame.passes[0];
    content->width = content->height = SIDE;
    content->samples = 1;
    content->count = 2;
    content->draws[0] = (struct draw){
        .rect = {31.0f, 21.0f, 62.0f, 42.0f},
        .kind = KIND_SOLID,
        .color = {0.82257867f, 0.27902827f, 0.66923934f, 1.0f},
        .blend = BLEND_OVER,
        .samples = -1,
    };
    content->draws[1] = (struct draw){
        .rect = {10.0f, 23.0f, 44.0f, 64.0f},
        .kind = KIND_SOLID,
        .color = {0.1675554f, 0.4423193f, 0.4975624f, 0.77261364f},
        .blend = BLEND_OVER,
        .samples = -1,
    };

    // One axis each, which is what makes a blur two passes.
    for (int axis = 0; axis < 2; axis++) {
        struct pass *blur = &frame.passes[1 + axis];
        blur->width = blur->height = SIDE;
        blur->samples = 1;
        blur->count = 1;
        blur->draws[0] = (struct draw){
            .kind = KIND_BLUR,
            .to_local = WHOLE,
            .step = {axis == 0 ? step : 0.0f, axis == 0 ? 0.0f : step},
            .sigma = 3.1289234f,
            .blend = BLEND_SRC,
            .samples = axis,
        };
        memcpy(blur->draws[0].rect, whole, sizeof whole);
    }

    struct pass *root = &frame.passes[3];
    root->width = root->height = SIDE;
    root->samples = 1;
    root->clear[3] = 1.0f;
    root->count = 2;
    root->draws[0] = (struct draw){
        .rect = {6.0f, 26.0f, 49.0f, 43.0f},
        .kind = KIND_SOLID,
        .color = {0.5475806f, 0.3393603f, 0.1716616f, 1.0f},
        .blend = BLEND_OVER,
        .samples = -1,
    };
    // The composite, and the whole subject of this file: the finished layer
    // drawn as a texture under the equation, at the layer's own opacity.
    root->draws[1] = (struct draw){
        .kind = KIND_IMAGE,
        .to_local = WHOLE,
        .alpha = 0.6805242f,
        .blend = BLEND_ADVANCED,
        .equation = equation,
        .samples = 2,
    };
    memcpy(root->draws[1].rect, whole, sizeof whole);
    return frame;
}

// The mapping a bounded layer composites through, which the recording carries
// for the third of the other frames: its target is wider than its layer.
static const float BOUNDED[12] = {2.064516f, 0.0f,  0.0f, 0.0f,
                                  0.0f,      -2.56f, 0.0f, 0.0f,
                                  0.9032258f, 1.88f, 1.0f, 0.0f};

// The three frames between the subject's two renders. What they owe is an
// advanced blend and more than one sample; which equation does not matter.
static void others(struct frame out[3]) {
    memset(out, 0, 3 * sizeof *out);

    out[0].count = 1;
    out[0].passes[0] = (struct pass){
        .width = SIDE, .height = SIDE, .samples = 1,
        .clear = {0.0f, 0.0f, 0.0f, 1.0f},
        .count = 2,
        .draws = {{
                      .rect = {31.0f, 17.0f, 54.0f, 62.0f},
                      .kind = KIND_SOLID,
                      .color = {0.34063724f, 0.09357163f, 0.6587454f,
                                0.22730592f},
                      .blend = BLEND_OVER,
                      .samples = -1,
                  },
                  {
                      .rect = {14.0f, 39.0f, 71.0f, 55.0f},
                      .kind = KIND_SOLID,
                      .color = {0.25314078f, 0.8680544f, 0.6820076f, 1.0f},
                      .blend = BLEND_SRC,
                      .samples = -1,
                  }},
    };

    out[1].count = 1;
    out[1].passes[0] = (struct pass){
        .width = SIDE, .height = SIDE, .samples = 4,
        .clear = {0.0f, 0.0f, 0.0f, 1.0f},
        .count = 2,
        .draws = {{
                      .rect = {9.0f, 33.0f, 25.0f, 71.0f},
                      .kind = KIND_SOLID,
                      .color = {0.29656017f, 0.033283897f, 0.3366662f,
                                0.8946956f},
                      .blend = BLEND_OVER,
                      .samples = -1,
                  },
                  {
                      .rect = {22.0f, 27.0f, 54.0f, 56.0f},
                      .kind = KIND_SOLID,
                      .color = {0.70035374f, 0.59060365f, 0.28776625f, 1.0f},
                      .blend = BLEND_OVER,
                      .samples = -1,
                  }},
    };

    // The one that carries the advanced blend: a small multisampled pass
    // composited into a frame of its own.
    out[2].count = 2;
    out[2].passes[0] = (struct pass){
        .width = 31, .height = 25, .samples = 4,
        .count = 1,
        .draws = {{
            .rect = {0.0f, 0.0f, 31.0f, 25.0f},
            .kind = KIND_SOLID,
            .color = {0.24970609f, 0.28284502f, 0.2957505f, 1.0f},
            .blend = BLEND_ADVANCED,
            .equation = GL_DIFFERENCE_KHR,
            .samples = -1,
        }},
    };
    out[2].passes[1] = (struct pass){
        .width = SIDE, .height = SIDE, .samples = 1,
        .clear = {0.0f, 0.0f, 0.0f, 1.0f},
        .count = 1,
        .draws = {{
            .rect = {36.0f, 17.0f, 67.0f, 42.0f},
            .kind = KIND_IMAGE,
            .to_local = BOUNDED,
            .alpha = 0.9840544f,
            .blend = BLEND_OVER,
            .samples = 0,
        }},
    };
}

static GLuint placeholder_texture(void) {
    const unsigned char white[4] = {255, 255, 255, 255};
    GLuint texture = 0;
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 1, 1, 0, GL_RGBA,
                 GL_UNSIGNED_BYTE, white);
    // A texture with the default mipmap filter and no mipmaps is incomplete and
    // samples as black, which is what the placeholder exists to avoid.
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    return texture;
}

// FNV-1a over a rendering, so a shader edit can be shown to have left the
// picture alone.
//
// Shrinking the fragment program is how a report gets a minimal one, and most
// of what can be cut is unreachable for these three materials rather than
// unused by the compiler -- the kind a draw takes is a uniform, so every arm is
// live until the driver specializes it. A cut that keeps this number keeps the
// frame, and then the only thing that moved is what the compiler was given.
static unsigned long long picture(const unsigned char *pixels) {
    unsigned long long hash = 1469598103934665603ULL;
    for (size_t i = 0; i < (size_t)SIDE * SIDE * 4; i++) {
        hash = (hash ^ pixels[i]) * 1099511628211ULL;
    }
    return hash;
}

// Where two renderings differ, and how. A one-sided difference in one channel
// is a different claim from a wobble: rounding scatters both ways.
static int report(const unsigned char *first, const unsigned char *again,
                  int announce) {
    const size_t bytes = (size_t)SIDE * SIDE * 4;
    int worst = 0;
    size_t differing = 0, lower = 0, higher = 0, channels[4] = {0};
    size_t at = 0;
    for (size_t i = 0; i < bytes; i++) {
        int delta = (int)first[i] - (int)again[i];
        int magnitude = delta < 0 ? -delta : delta;
        if (magnitude > worst) {
            worst = magnitude;
            at = i;
        }
        if (delta != 0) {
            differing++;
            channels[i % 4]++;
            if (delta > 0) {
                lower++;
            } else {
                higher++;
            }
        }
    }
    printf("worst %d level(s), %zu of %zu byte(s) differ\n", worst, differing,
           bytes);
    // AT=x,y prints that pixel from both renderings whether or not they differ,
    // which is what a clean driver has to be asked for: its answer is the one
    // the two are measured against, and it has no difference to report.
    const char *asked = getenv("AT");
    if (asked) {
        unsigned x = 0, y = 0;
        if (sscanf(asked, "%u,%u", &x, &y) == 2 && x < SIDE && y < SIDE) {
            size_t pixel = ((size_t)y * SIDE + x) * 4;
            printf("  at (%u,%u): first [%3u,%3u,%3u,%3u] "
                   "again [%3u,%3u,%3u,%3u]\n",
                   x, y, first[pixel], first[pixel + 1], first[pixel + 2],
                   first[pixel + 3], again[pixel], again[pixel + 1],
                   again[pixel + 2], again[pixel + 3]);
        }
    }
    if (worst > 0 && announce) {
        printf("  the second render is lower in %zu and higher in %zu; "
               "by channel r%zu g%zu b%zu a%zu\n",
               lower, higher, channels[0], channels[1], channels[2],
               channels[3]);
        size_t pixel = (at / 4) * 4;
        printf("  worst at (%zu,%zu): first [%3u,%3u,%3u,%3u] "
               "again [%3u,%3u,%3u,%3u]\n",
               (at / 4) % SIDE, (at / 4) / SIDE, first[pixel], first[pixel + 1],
               first[pixel + 2], first[pixel + 3], again[pixel],
               again[pixel + 1], again[pixel + 2], again[pixel + 3]);
    }
    return worst;
}

int main(int argc, char **argv) {
    // `--generated <vertex> <fragment>` reads the renderer's own pair from
    // files instead of using the two above. It is how the two programs are
    // compared, and the probe this was reduced from prints them.
    const char *vertex_path = NULL;
    const char *fragment_path = NULL;
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--generated") == 0) {
            if (argc - i < 3) {
                fprintf(stderr, "--generated takes a vertex and a fragment "
                                "path\n");
                return 1;
            }
            vertex_path = argv[i + 1];
            fragment_path = argv[i + 2];
            i += 2;
        } else {
            fprintf(stderr, "unknown argument %s\n", argv[i]);
            return 1;
        }
    }

    const char *runs_env = getenv("RUNS");
    int runs = runs_env ? atoi(runs_env) : 12;
    if (runs < 1) {
        runs = 1;
    }
    // How many times the three other frames run between the subject's two
    // renders. One is what the renderer does and is enough with the shaders
    // above; the generated pair needs more, and the header says what that
    // measured.
    const char *rounds_env = getenv("ROUNDS");
    int rounds = rounds_env ? atoi(rounds_env) : 1;
    if (rounds < 1) {
        rounds = 1;
    }
    const char *equation_env = getenv("EQUATION");
    GLenum equation =
        equation_env ? (GLenum)strtoul(equation_env, NULL, 0) : GL_OVERLAY_KHR;
    // The blur's half-width, which decides the loop's trip count. Five is what
    // the frame this was reduced from computes from its own deviation, and the
    // header says what the knob is for: it moves the error across the eight-bit
    // floor without changing what the driver compiles.
    const char *taps_env = getenv("TAPS");
    int taps = taps_env ? atoi(taps_env) : 5;
    if (taps < 1) {
        taps = 1;
    }

    char *built = NULL;
    const char *vertex_source = VERTEX;
    const char *fragment = NULL;
    if (fragment_path) {
        vertex_source = read_file(vertex_path, NULL);
        fragment = read_file(fragment_path, NULL);
    } else {
        // The tap count is a literal in the shader rather than a uniform, so it
        // is substituted here.
        size_t room = strlen(FRAGMENT) + 32;
        built = malloc(room);
        if (!built || snprintf(built, room, FRAGMENT, taps, taps) < 0) {
            return 1;
        }
        fragment = built;
    }
    // Both forms, as the renderer builds both: the qualified one for the draws
    // under an advanced equation and the plain one for everything else.
    char *blend_fragment = qualified(fragment);

    struct frame subject_frame = subject(equation);
    struct frame other_frames[3];
    others(other_frames);

    unsigned char *first = malloc((size_t)SIDE * SIDE * 4);
    unsigned char *again = malloc((size_t)SIDE * SIDE * 4);
    unsigned char *discard = malloc((size_t)SIDE * SIDE * 4);
    if (!first || !again || !discard) {
        return 1;
    }

    int hits = 0;
    for (int run = 0; run < runs; run++) {
        // A context per run: what is being measured is what one frame leaves
        // for the next, so a shared context would carry the run before it as
        // much as the run.
        EGLDisplay display = eglGetPlatformDisplay(
            EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL);
        if (display == EGL_NO_DISPLAY) {
            fprintf(stderr, "no surfaceless EGL display\n");
            return 1;
        }
        if (!eglInitialize(display, NULL, NULL) ||
            !eglBindAPI(EGL_OPENGL_ES_API)) {
            fprintf(stderr, "cannot initialize EGL\n");
            return 1;
        }
        const EGLint config_attrs[] = {EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
                                       EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
                                       EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8,
                                       EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8,
                                       EGL_NONE};
        EGLConfig config;
        EGLint found = 0;
        if (!eglChooseConfig(display, config_attrs, &config, 1, &found) ||
            found != 1) {
            fprintf(stderr, "no ES3 RGBA8 config\n");
            return 1;
        }
        const EGLint context_attrs[] = {EGL_CONTEXT_MAJOR_VERSION, 3,
                                        EGL_CONTEXT_MINOR_VERSION, 0, EGL_NONE};
        EGLContext context =
            eglCreateContext(display, config, EGL_NO_CONTEXT, context_attrs);
        if (context == EGL_NO_CONTEXT ||
            !eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context)) {
            fprintf(stderr, "cannot make a context current\n");
            return 1;
        }

        const char *extensions = (const char *)glGetString(GL_EXTENSIONS);
        if (!extensions ||
            !strstr(extensions, "GL_KHR_blend_equation_advanced")) {
            fprintf(stderr, "skipping: no GL_KHR_blend_equation_advanced\n");
            return 0;
        }
        barrier_fn barrier = NULL;
        if (!strstr(extensions, "GL_KHR_blend_equation_advanced_coherent")) {
            barrier = (barrier_fn)eglGetProcAddress("glBlendBarrierKHR");
            if (!barrier) {
                fprintf(stderr, "skipping: no glBlendBarrierKHR\n");
                return 0;
            }
        }
        if (run == 0) {
            printf("renderer: %s\n", glGetString(GL_RENDERER));
            printf("version:  %s\n", glGetString(GL_VERSION));
            if (fragment_path) {
                printf("shaders:  %s and %s\n", vertex_path, fragment_path);
            } else {
                printf("shaders:  the fifty lines above, %d taps\n", taps);
            }
        }

        struct program program = {0};
        program.plain = link_program(vertex_source, fragment);
        program.blend = link_program(vertex_source, blend_fragment);
        program.image_plain = sampler_of(program.plain);
        program.image_blend = sampler_of(program.blend);
        glGenVertexArrays(1, &program.vao);
        glGenBuffers(1, &program.vertices);
        glGenBuffers(1, &program.indices);
        glGenBuffers(1, &program.paints);
        GLint alignment = 1;
        glGetIntegerv(GL_UNIFORM_BUFFER_OFFSET_ALIGNMENT, &alignment);
        if (alignment < 1) {
            alignment = 1;
        }
        const GLsizei bytes = MATERIAL_FLOATS * (GLsizei)sizeof(float);
        program.stride = ((bytes + alignment - 1) / alignment) * alignment;
        GLuint placeholder = placeholder_texture();

        render(&program, placeholder, barrier, &subject_frame, first);
        for (int round = 0; round < rounds; round++) {
            for (int i = 0; i < 3; i++) {
                render(&program, placeholder, barrier, &other_frames[i],
                       discard);
            }
        }
        render(&program, placeholder, barrier, &subject_frame, again);

        if (run == 0) {
            printf("picture:  %016llx\n\n", picture(first));
        }
        printf("run %2d: ", run + 1);
        if (report(first, again, runs == 1) > 0) {
            hits++;
        }

        eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        eglDestroyContext(display, context);
        eglTerminate(display);
    }
    if (runs > 1) {
        printf("\n%d hit(s) of %d\n", hits, runs);
    }
    return 0;
}
