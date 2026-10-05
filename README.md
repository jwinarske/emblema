# emblema

A tessellation-based 2D vector graphics renderer for Rust, targeting everything
from desktop discrete GPUs down to embedded SoCs driving panels directly
through KMS with no compositor present.

> **Status: early development.** Solid fills, strokes, antialiasing, gradients
> (linear, radial, sweep), and the Porter-Duff blend modes render on both
> backends and are verified against real hardware and a software reference. The
> advanced blend modes — separable and non-separable both, all fifteen — render
> on Vulkan where the device offers the advanced-blend extension, and are
> reported as unavailable elsewhere rather than approximated. Clipping works on both
> backends for rectangles and arbitrary paths, nests, and antialiases with the
> shapes it confines. Images can be uploaded and drawn as a paint, with clamp,
> repeat, mirror and decal tiling, and with a source rectangle so a sprite
> sheet or a nine-patch is one upload, and a tint so one monochrome icon sheet
> serves every state, and gradients tile the same four ways past
> their own extent and take any number of color stops. Strokes dash, with the
> pattern measured along the path so it follows a curve rather than its chord.
> Paths take elliptical arcs, which is what a progress ring and a pie slice are
> made of. Save layers give a subtree its own target, so group
> opacity and layer-wide blend modes work and nest, and a layer can blur what is
> behind it -- frosted glass, not just a soft shadow. A glyph atlas packs
> caller-supplied single-channel coverage, evicts what a frame stops using,
> grows when it has nothing to evict, and draws a run of any length as one
> draw; bring your own rasterizer. Windowed presentation works on both backends
> and takes the same shape on each: the caller creates the surface — a
> `VkSurfaceKHR` or an `EGLSurface` — because owning that relationship would
> mean owning a windowing library. What no test here can see is what a window
> system actually puts on screen, so both paths are exercised against a surface
> with no window behind it and pin the property that decides it instead.

Fill is where the time goes on the devices this targets, and two changes to it are
measured rather than argued. A rectangle's fill is tessellated rather than evaluated
as a distance field, which took thirty-four per cent off an interface frame on a
Raspberry Pi 5 and thirty on a VisionFive 2 — the opposite of what the code
assumed. And a draw is now confined to the pixels no later opaque draw replaces,
which took two thirds off that same frame: 16.07 ms to 5.19 on V3D, 27.2 to 11.0 on
an Adreno 640. Neither changes a picture; both are in
[`docs/on-a-board.md`](https://github.com/jwinarske/emblema/blob/main/docs/on-a-board.md)
with the conditions they were taken under.

## Why

- **Tessellation-based, no compute shader requirement.** Runs correctly on
  every Vulkan 1.1+ device and every GLES 3.0 device, including embedded GPUs
  where compute is weak or driver support is immature.
- **Predictable frame times.** All pipelines are compiled ahead of time — no
  shader compilation jank, no driver-specific compute paths.
- **First-class direct scanout.** Most 2D renderers assume a windowing system
  exists. Here, rendering straight to a KMS plane with no compositor is a
  supported and tested configuration rather than an exercise left to the
  reader.
- **Pure-Rust source tree.** Every dependency compiles from Rust source. No C
  or C++ toolchain, no pkg-config, no system headers at build time, which keeps
  cross-compilation to aarch64 and riscv64 boards trivial.

## The shape of it

Two orthogonal axes. The **rendering HAL** answers how draw commands become
pixels in a GPU image. **Presentation** answers how a finished image reaches
the display, and how the frame loop is paced. DRM/KMS is a presentation target
alongside windowed surfaces, not a third rendering backend.

|            | WSI (windowed)      | DRM (direct scanout)                          |
|------------|---------------------|-----------------------------------------------|
| **Vulkan** | `VkSwapchainKHR`    | VkImage → dma-buf export → drm-rs FB → commit |
| **GLES**   | EGL window surface  | *planned* — EGL on GBM → gbm_surface → drm-rs FB → commit |

**Three of the four are built.** GLES to direct scanout is not: there is no `gbm`
anywhere in the tree, no dependency and no code, and that cell describes a route
that does not exist yet. This table claimed all four until the code was read
against it.

The windowed column works today against a surface
the caller supplies. The scanout column is **partial**: the frame loop above
KMS is implemented and tested — the buffer ring, fence plumbing, and format and
modifier negotiation — and dma-buf export from Vulkan is real. A buffer this
renderer allocates, draws into and exports is accepted by a display controller as
a framebuffer, the mode is set, and frames flip in turn.

That is checked two ways rather than one. On a workstation it is the virtual KMS
driver, because a compositor holds master on any card driving a display. On a
Raspberry Pi 5 it is the hardware: no display server runs there, so the tests can
take master, and all thirty-nine in `emblema-present-drm` pass there — including the
six that take master and set a mode and commit a frame — on `vc4` with `v3d` as a
separate render node, which is the split render and display topology the virtual
driver stands in for. Re-run on 2026-10-03; nothing in the three device binaries
reported a skip, which matters because a skipped test passes and would read the
same from here. The render fence rides each commit, so the kernel latches the flip when
rendering completes and the frame loop blocks on nothing, except on a commit that
also sets the mode: the virtual driver will not complete one with a fence attached,
which happens once per output.

What keeps the column partial is therefore not the absence of real hardware. It is
what neither lane reaches — writeback and CRC readback, resize storms, hotplug —
and the quirks only a rack of boards finds: IOMMU faults, compressed-format corner
cases, scaler limits.

And one structural limit, which is the same one the GLES cell runs into:
**this renderer allocates the scanout buffer and asks the display to accept it.**
Where a display controller cannot import what the GPU chose, there is no fallback
and the target refuses. That is measured on two boards — a Pi 4, whose display
controller will not import render-node memory without an IOMMU, and an i.MX8M
Plus, whose Vulkan export is scatter-gather where the controller needs it
contiguous. [`docs/architecture.md`](https://github.com/jwinarske/emblema/blob/main/docs/architecture.md)
has the comparison that named it.

`cargo xtask drm` says whether a given machine could run that lane.

```rust
use emblema::{BackendPreference, Canvas, Color, Context, Extent2D, Paint, PixelFormat, Rect};

// The backend is chosen at run time, so one binary serves a board with a
// working Vulkan driver and one where only GLES is usable.
let mut ctx = Context::new(BackendPreference::Auto)?;

let size = Extent2D::new(256, 256);
let mut surface = ctx.create_surface(size, PixelFormat::Rgba8Unorm)?;

let mut canvas = Canvas::new(size);
canvas.clear(Color::WHITE);
canvas.draw_rect(
    Rect::new(32.0, 32.0, 224.0, 224.0),
    &Paint::fill(Color::rgba8(0, 120, 220, 255)),
)?;

// Recording is separate from submitting, so a whole frame is described
// before any of it reaches the GPU and every shape shares one pass.
ctx.draw(&mut surface, &canvas.finish())?;
let pixels = ctx.read(&mut surface)?;

// Destroyed explicitly, because nothing here frees a GPU object on drop. A
// surface still alive when its context goes is a child outliving its device,
// which the validation layer reports as an error rather than a leak.
ctx.destroy_surface(surface);
```

## Running it

```sh
cargo run -p emblema --example frame -- frame.ppm
```

Draws one frame through the public API — a gradient, a group composited through
a layer, an image sampled through a path clip, a blend mode, and a run of
glyphs — and writes a binary PPM, which needs no encoder. Image encoding is out
of scope, so converting to something friendlier is `magick frame.ppm frame.png`.

## How much of a renderer this is

[`docs/parity.md`](https://github.com/jwinarske/emblema/blob/main/docs/parity.md) is the operation-by-operation comparison
against the `dart:ui` `Canvas` and `Paint` surface — the contract a
Flutter-class renderer owes, and a more useful yardstick than any one
implementation's internals. It distinguishes what exists from what a caller
could assemble, and every row claiming something works names the scene or test
that renders it.

[`docs/non-parity.md`](https://github.com/jwinarske/emblema/blob/main/docs/non-parity.md) is the other half of that question.
Upstream Impeller is what this project decides against for *what* a renderer
must express, and not for *how* it is reached: strategy answers to the devices
this one targets, which are not the devices upstream targets. So the places the
two knowingly differ are worth finding in one list rather than inferring from a
diff. Each entry says what differs, why, and what the difference costs. A filled
path is the clearest — stencil-then-cover there, triangulated here, so that one
binary serves any Vulkan 1.1 or GLES 3.0 device. The deepest is that this
pipeline works in linear light and upstream's does not, which is why a
comparison against upstream can only be a comparison of shape wherever two
colors are mixed.

## Building

Requires a Rust toolchain; no other build dependencies. MSRV is 1.85, which is
where edition 2024 stabilized and is a floor the dependency graph sets rather
than the code: naga translates the shaders at build time and reaches indexmap.
CI builds the workspace on exactly that toolchain, so this figure is checked.

`rust-toolchain.toml` pins the compiler this is *built* with to 1.94.1, which
is a separate question from the floor and answered from a different place: it
is the version Yocto's wrynose release ships, so a build here uses the compiler
an embedded target will. The floor stays lower deliberately — at 1.85 it also
admits whinlatter's 1.90, which is checked. Walnascar's 1.84.1 misses by one
release on a dependency's edition, and scarthgap's 1.75 would cost a `naga`
downgrade that changes the emitted SPIR-V; `docs/architecture.md` has the
measurements.

```sh
cargo build                                   # default: vulkan + WSI
cargo build --features gles,present-egl       # GLES into an EGL window
cargo build --features gles,drm               # the classic embedded GBM path
cargo build --features vulkan,gles,drm        # one binary that picks at runtime
cargo test
```

The two axes are named separately because they are separate. `drm` names no
backend and composes with either, which is the orthogonality the whole design
rests on. `present-wsi` and `present-egl` each imply a backend, not because the
axes are coupled but because a Vulkan swapchain is Vulkan's own object and an
EGL surface is GLES's.

A test that finds no device prints why and returns, and `cargo test` captures
the output of a passing test — so a run with nothing to run on looks exactly
like a run that covered everything. `cargo xtask verify` runs the same suite
and prints what did *not* run, which is the number worth reading.

`drm` is presentation-only and composes with either rendering backend. Feature
flags live on the `emblema` facade crate, since a virtual workspace root
cannot declare them.

Development tasks run through `cargo xtask`. `report` says what this machine's
devices can do, `drm` whether it can drive a display directly, `verify` runs
the suite and reports what did *not* run, and `gallery` renders every corpus
scene onto one sheet to look at. `cargo xtask help` lists them.

## Layout

```
crates/
  emblema               public facade; carries the feature flags
  emblema-core         drawing API: Canvas, Paint, Color, recording, execution
  emblema-entity       entity and contents layer -- coverage only, not yet routed
  emblema-geometry     path types, flattening, tessellation, stroking, dashing
  emblema-renderer     render pass encoding, generic over the HAL
  emblema-text         glyph atlas packing and placement; bring your own rasterizer
  emblema-hal          rendering HAL trait
  emblema-hal-vulkan   Vulkan backend (first-class)
  emblema-hal-gles     GLES 3.0 backend
  emblema-present      presentation trait, format negotiation
  emblema-present-vk   Vulkan WSI swapchain target
  emblema-present-egl  EGL window-surface target
  emblema-present-drm  DRM/KMS scanout target
  emblema-shaders      WGSL sources, build-time translation via naga
  emblema-testkit      shared test harness
  emblema-capi         Impeller C API (libimpeller), ABI-compatible
xtask/                  device runs, golden management, CI reproduction
docs/                   architecture
```

## Impeller C API

`emblema-capi` builds `libimpeller`, intended as an ABI-compatible
implementation of upstream Impeller's C API so that a consumer linking that API
could link this instead without recompiling. **It is barely started**: version
negotiation is the only entry point, and the rest waits on a vendored copy of
the upstream header — guessing an enum value or a struct layout would produce a
library that links and then corrupts memory.

It is **not** a drop-in for Impeller inside the Flutter Engine build: the
engine compiles Impeller's C++ sources directly rather than consuming them
across this boundary, and no Rust library can present a compatible C++ ABI. The
C API serves embedders. See
[`docs/architecture.md`](https://github.com/jwinarske/emblema/blob/main/docs/architecture.md#emblema-c-api-compatibility) for
where parity is partial and how it is verified.

## Scope

Bring your own text shaping and layout (`cosmic-text`, `parley`), font parsing
(`ttf-parser`, `swash`), image decoding (`image`), and SVG parsing (`usvg`).
Scene graph, retained mode, animation, and 3D are out of scope, as is GLES 2.0.

This is a renderer, not a compositor: it draws to planes it is given.
Multi-client composition belongs elsewhere. KMS internals belong outside it,
and which outside differs by concern: connector and plane probing is drm-rs's,
while EDID parsing, mode selection policy, session and seat handoff, and
hotplug detection are the application's or its session manager's — drm-rs
reports the modes but does not choose one, and hotplug needs udev, which
nothing here uses.

## Documentation

[`docs/architecture.md`](https://github.com/jwinarske/emblema/blob/main/docs/architecture.md) covers the design and the rules
that govern the codebase: the HAL and presentation split, the Vulkan-first
policy, explicit synchronization, the ownership boundary with drm-rs, format
and modifier negotiation, the shader pipeline, dependency purity, and the
testing model. Read it before proposing structural changes — a fair number of
alternatives were considered and rejected for recorded reasons.

[`docs/on-a-board.md`](https://github.com/jwinarske/emblema/blob/main/docs/on-a-board.md) is how to cross-build the suite and
run it on a real device, and what doing so has found. Most of what this suite
checks is agreement between two devices, and on a workstation both of them are
software — so the board is not a nice-to-have lane, it is where a class of
defect is visible at all.

## Releases

**No release under this name has been published yet.** `emblema` and the
thirteen crates it is assembled from hold `0.0.0` and `0.0.1` on crates.io, and
both contain **no API**: they reserved the names ahead of the first real release
under them. Nothing should depend on either.

The prepared release is `0.2.0`, and until it is pushed the dependency below
will not resolve. `cargo xtask release` prints the order it goes up in and what
each step needs.

```toml
[dependencies]
emblema = "0.2.0"
```

What *is* on the registry is the old name: `impeller-rs 0.1.0` and thirteen
`impeller-*` crates, published 2026-09-21 before the rename. They are a snapshot
of this project from that day and will not move again; crates.io does not delete,
so they stay as a record. A hundred and two merges have landed since.

`emblema`'s default features are Vulkan and its swapchain, but docs.rs builds it
with `all-features`, so the GLES backend and both presentation paths are documented
there whatever a caller enables. The links in this file are absolute for a
related reason -- a relative one resolves here and 404s on a crate page.


Fourteen of this workspace's seventeen crates publish; `emblema-capi`,
`emblema-testkit` and `xtask` refuse, each saying why in its own manifest
and each one line from changing its mind. A release is ordered, because
`cargo publish` verifies a packaged crate against the registry rather than against
the workspace: nothing can go up before what it depends on, and there is no way to
rehearse the whole sequence in advance. A crate published for the first time also
spends a token from a bucket that holds five and refills one every ten minutes, so
adding several new crates at once waits on the clock rather than on this
repository. [`CHANGELOG.md`](https://github.com/jwinarske/emblema/blob/main/CHANGELOG.md)
records what each version carried.

## Contributing

Run these in order before committing, and do not commit on a failure:

```sh
cargo clippy --workspace --all-targets --fix --allow-dirty   # lint, applying fixes
cargo fmt --all                                              # format
cargo build --workspace --all-targets                        # smoke test
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps   # resolve doc links
cargo test --workspace
```

`cargo xtask gate` is all of that as one exit code. Read the test count rather
than the exit code when running the pieces by hand: a suite that compiled
nothing and a suite that passed everything both exit zero.

It also prints what the suite said about its own coverage -- how many catalog
plates were drawn and how many compared -- because those are different numbers
and only one of them is in the total. A plate needing a capability one backend
lacks is reported and skipped, correctly, and the suite still passes; "834
passed" reads the same whether it compared every plate or four fifths of them.

Read `gate`'s own output whole, though, and not through a filter. Test binaries
run in parallel and one can write over another's summary line; the count is then
short by however many tests that binary held, and by its failures too. That is
detected and said out loud -- "N test binaries said nothing this could read" --
and the run fails. But it is said in a line that a grep for `passed` or `FAILED`
does not match, and a pipe hides the exit code that would have caught it. A
count four short with no failures and no skips is what that looks like from
behind a narrow filter.

**A green gate is not a green CI, and the difference is not cosmetic.** CI runs
the same suite against lavapipe with software GL forced, and with the Vulkan
validation layer installed. Two things follow. A device the machine in front of
you does not have will disagree — one lavapipe version writes outside a scissor
where three other drivers do not, and a threshold fitted to one GPU can clear it
by one per cent there and fail everywhere else. And the validation layer is the
only thing that reports a Vulkan object outliving its device; without it those
tests still pass, and on a machine that has no layer they say so in skips nobody
reads. A hundred and forty-nine of them, measured on 2026-09-02 rather than
estimated, and measurable again in one command:

```sh
mkdir -p /tmp/no-layers && VK_LAYER_PATH=/tmp/no-layers cargo xtask verify
```

Which hides the layer's manifest from the loader and leaves everything else
alone: the same 954 tests pass either way, with six skips when the layer is
there and a hundred and fifty-five when it is not. This said a hundred and
twenty once and the suite outgrew it, which is what the command is for.

The gate says what CI last said, in a line beside the skip census and the
timing drift, and for the same reason both of those are there: it is something
this machine cannot check, so it is reported rather than enforced. It went
unreported once and CI stayed red for twenty-six commits on one assertion while
every local gate passed. No `gh`, no network or no run is itself a line -- the
outcome this must not have is silence.

To run what CI runs:

```sh
cargo xtask verify --software
```

Which finds the CPU drivers wherever the distribution put them and prints the
variables it set, so the run says which drivers answered it. `gate` takes the
flag too.

The feature axes are meant to compose independently, so check that they still
do — a backend and a presentation path are orthogonal, and a combination that
only builds because another feature happened to be on is a coupling:

```sh
for f in vulkan gles vulkan,gles,drm present-wsi gles,present-egl vulkan,gles,drm,present-wsi,present-egl; do
  cargo check -p emblema --no-default-features --features "$f" || break
done
```

Two conventions about writing rather than building. American English
throughout — code, comments, documentation, commit messages. And a commit
message should explain why a change is shaped the way it is rather than
restate what moved, in prose paragraphs rather than bullet lists; the diff
already says what changed, and the reasoning is the part that is expensive to
recover later.

## License

BSD 3-Clause. See [`LICENSE`](https://github.com/jwinarske/emblema/blob/main/LICENSE).

This matches the Flutter Engine, home of the C++ Impeller whose architecture
this project takes as its reference. Any code ported from there retains its
original copyright notice alongside this project's.
