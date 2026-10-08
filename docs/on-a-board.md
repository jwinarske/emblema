# Running the suite on a board

A workstation cannot answer the question this suite is mostly built to ask.
`conformance` renders every corpus scene on two devices and requires them to
agree, which is the check that scales: adding a scene extends coverage across
every device the corpus runs on without anyone certifying a reference image.
On a machine whose only Vulkan devices are llvmpipe and a software reference,
those two devices are the same kind of thing, and a difference that only a
real driver produces cannot appear.

A Raspberry Pi 5 has both halves: v3d, which is a tiler with a real shader
compiler, and llvmpipe beside it. Pointing `conformance` at that pair is what
the following is for.

## Cross-building without a target libc

Fedora's `gcc-aarch64-linux-gnu` installs a compiler and leaves
`/usr/aarch64-linux-gnu/sys-root` empty, so compilation succeeds and linking
fails on everything at once — `cannot find -lc`, `cannot find crtn.o`. That
reads as a broken toolchain and is not one; there is simply nothing to link
against.

Take the sysroot from the board. It runs the exact glibc, gcc runtime and
kernel headers the binary will meet, which makes it a better sysroot than a
packaged one and a worse thing to forget you depend on. About twenty-seven
megabytes, once.

`PI` is whichever name your `known_hosts` carries the board's *current* key under,
which is not always the one written here. This board's host key has been regenerated
at least once, and the entries left behind do not all point at the same key: on one
workstation `raspberrypi.local` and the board's address carry the current key while
`raspberrypi.lan` still holds the old one, so `ssh joel@raspberrypi.lan` fails with
`REMOTE HOST IDENTIFICATION HAS CHANGED` and `ssh joel@raspberrypi` fails with
`Host key verification failed` for the plainer reason that no entry exists under the
bare name.

Neither is an attack and neither is a reason to pass `StrictHostKeyChecking=no`.
Fingerprint what the host offers, compare it against the entries you already trust,
and use a name that matches:

```sh
ssh-keyscan -t ed25519 raspberrypi.local 2>/dev/null | ssh-keygen -lf -
```

If that fingerprint is already in `known_hosts` under another name, the key is one
you have trusted before and the stale entry is the thing to fix. If it is not, stop
and find out why before typing a password at it.

```sh
S=$HOME/.cache/pi-sysroot
PI=joel@raspberrypi.local

mkdir -p "$S/usr/lib/aarch64-linux-gnu" "$S/usr/lib/gcc"
rsync -a \
  --include='*.o' --include='*.a' \
  --include='libc.so*' --include='libm.so*' --include='libmvec.so*' \
  --include='libdl.so*' --include='libpthread.so*' --include='librt.so*' \
  --include='libutil.so*' --include='libanl.so*' --include='libgcc_s.so*' \
  --include='ld-linux-aarch64*' --exclude='*' \
  "$PI:/usr/lib/aarch64-linux-gnu/" "$S/usr/lib/aarch64-linux-gnu/"
rsync -a "$PI:/usr/lib/gcc/aarch64-linux-gnu" "$S/usr/lib/gcc/"

ln -sfn usr/lib "$S/lib"
ln -sfn aarch64-linux-gnu/ld-linux-aarch64.so.1 "$S/usr/lib/ld-linux-aarch64.so.1"
```

Include `*.o` rather than `crt*.o`: the glob is case-sensitive, and `Scrt1.o` —
the start file for a position-independent executable — is what Rust asks for.

```sh
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="\
-C link-arg=--sysroot=$S \
-C link-arg=-B$S/usr/lib/aarch64-linux-gnu \
-C link-arg=-L$S/usr/lib/aarch64-linux-gnu \
-C link-arg=-L$S/usr/lib/gcc/aarch64-linux-gnu/14"

cargo test --workspace --exclude xtask --target aarch64-unknown-linux-gnu --no-run
```

All four link arguments earn their place, and each is missed differently:
without `--sysroot` the linker scripts resolve `/lib/...` against the host;
without `-B` the start files are not found; without the multiarch `-L`, `-lc`
is not found; without the gcc `-L`, `-lgcc_s` is not found. The `14` is gcc's
major version on the board — check it rather than assume.

Nothing above installs Vulkan, EGL, GBM or libdrm for the target, and nothing
needs to: `ash` opens the loader at runtime and `drm-rs` issues ioctls through
`rustix`, so neither reaches the linker.

## Running them there

`--no-run` prints each binary it built. Copy those across and run them; there is
no cargo on the board and none needed.

Ask cargo which ones they are rather than globbing the directory they landed in.
`deps/` also holds every build script and every stale artifact from earlier
builds: 1865 files and 5.0 GB here against 59 test binaries, and 179 of those
files are executable, so a glob copies eighty times what is needed and the loop
then runs 120 build scripts as though they were tests. It is not merely
wasteful. `/tmp` on a Pi 5 is a 4.0 GB tmpfs, so the copy cannot finish, and it
fails after filling the board's memory rather than at the start.

```sh
cargo test --workspace --exclude xtask --target aarch64-unknown-linux-gnu \
  --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys, json
for line in sys.stdin:
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("profile", {}).get("test") and m.get("executable"):
        print(m["executable"])' > /tmp/pibins.txt

ssh "$PI" 'rm -rf /tmp/pibins && mkdir -p /tmp/pibins'
tar cf - -C target/aarch64-unknown-linux-gnu/debug/deps $(sed 's#.*/##' /tmp/pibins.txt) \
  | ssh "$PI" 'cd /tmp/pibins && tar xf - && chmod +x *'
ssh "$PI" 'cd /tmp/pibins && for b in *; do ./$b --test-threads=1; done'
```

`profile.test` is the field that matters: it is what separates a test binary from
a build script, and both are `compiler-artifact` messages with an `executable`.
One `tar` through the pipe rather than `scp` per file, because 59 round trips to
a board over a home network is the slowest part of the whole exercise.

Two things the harness needs, both of which look like failures when missing.

`--test-threads=1`, for anything taking DRM master: two tests racing for it
report the second as unable to open a card, which reads as a device problem and
is a scheduling one.

`EMBLEMA_SHADER_SNAPSHOTS`, naming a directory holding `tests/shader-snapshots`
copied across. The snapshot tests find their files from `CARGO_MANIFEST_DIR`,
which is baked in at compile time and names a path on the machine that did the
compiling. Without it they are the only two tests here that cannot run from a
bare binary, and a board run can never come out clean.

`EMBLEMA_COST_BASELINE`, naming a copy of
`crates/emblema-testkit/tests/cost-baseline.txt`, for the same reason and with
the same failure. The cost table is counted rather than measured, so a board is
where the claim that it is device-independent gets tested against a different
architecture instead of a different driver -- which is worth doing and cannot
be done from a binary that looks for its baseline on the machine that compiled
it. Recorded on x86-64, it matched byte for byte on the Pi 5.

`EMBLEMA_WRITEBACK_DEVICE=auto` is what makes a board session of
`writeback.rs` worth more than the desktop one. The test renders through the
software device by default, because on this workstation's RADV the kernel's
read of an exported buffer is the frame only in its last stretch -- the section
below has the figures. That is a property of a controller compositing with the
CPU, which is what vkms does and what a display controller does not, so on a
board with a real one the reason for the default does not apply and a real GPU
export is the stronger thing to check. Unset, or `software`, keeps the default.
It is also the one-command reproduction of that finding here.

`EMBLEMA_DRM_CARD` matters on a board with more than one display controller. A
Pi 5 has two, and a test that opens the first `/dev/dri/cardN` gets `rp1-dsi`
rather than `vc4`. A suite that passes on the controller that works says nothing
about the one beside it; `emblema-present-drm`'s crate documentation has the
table of what each board actually does.

## Benchmarking there, which has two rules of its own

The suite above runs from a debug build and should. `cargo xtask bench` must
not, and refuses to: cross-build it with `--release` and copy that binary.

```sh
# with $S and the two CARGO_TARGET_* exports above already set
cargo build -p xtask --release --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/xtask "$PI:/tmp/xtask"
ssh "$PI" 'chmod +x /tmp/xtask && /tmp/xtask bench --skip llvmpipe'
```

The comment is there because this block looks self-contained and is not. Run it
in a fresh shell and the link fails four times over, each time differently and
each time in a way that reads as a broken cross toolchain: `incompatible with
elf64-x86-64` while the host linker is still being used, then `cannot find -ldl`,
then `cannot find -lgcc_s`, then `cannot find Scrt1.o`. Every one of those is
answered by the paragraph under the export block above, which is worth reading
before improvising a fix for any of them.

The reason is not that debug is slower. The benchmark compares a path that
submits a hundred and sixty draws against one that submits a single merged
draw, so unoptimized per-draw cost lands on one side of the comparison and not
the other. On this board that is the difference between a real number and a
five percent regression that is not there — see the distance-field section of
[`architecture.md`](architecture.md), which records how one was chased. The two
binaries are easy to tell apart when you are not sure which got copied: the
release one is about 1.7 MB and the debug one about 30 MB.

`--skip llvmpipe` leaves the software rasterizer unstarted. Measuring on it
holds all four of the Pi 5's cores flat out for minutes at 1920x1080, and the
board locked up at that stage four times in a row, each time having come
through V3D's configurations first — once leaving the DSI panel full white with
the network gone, which looks more like a kernel or display hang than a supply
that cannot hold up. Whether it is power or heat is not established: throttle
flags read clean beforehand and two of the four were from a cold boot. The
board's own GPU is the number worth having and it is measurable without ever
starting that stage.

**Pin the frequency governor before benching, or the numbers are the governor's.**
The Pi 5 defaults to `ondemand` over 1.5 to 2.4 GHz, and anything with processor
work in it reads whichever clock it caught:

```sh
for c in /sys/devices/system/cpu/cpu[0-9]*; do
  echo performance | sudo tee $c/cpufreq/scaling_governor
done
```

Measured 2026-09-18. The recording rows -- which are pure processor work -- landed
in two or three states under `ondemand`, up to forty per cent apart, and held
inside one per cent pinned. Two GPU rows moved with it as well, and they are the
two with enough processor work to notice: the cheap stroked path and the
twelve-draw frame, each about two per cent. Everything GPU-bound did not move.
Several four and five per cent GLES outliers that had been put down to contention
went away too.

It does not survive a reboot, so `cargo xtask bench` prints the governor in its
header and the baseline names the one it was recorded under. A run whose header
says `ondemand` is not comparable with that file.

**Pinning it also settled the Vulkan bimodality, which this document and the
baseline had both carried as unexplained.** Four Vulkan rows used to land in one of
two states from one process to the next, 4.3 per cent apart on the distance field;
ten pinned runs put all eighteen rows in one cluster each, the distance field
spreading 0.14 per cent. Nine unpinned runs had shown the slow state five times.
Heat and accumulated session state had both been recorded as the cause and both
been tested and disproved -- with a fan, a cold boot and a power cycle -- so the
lesson is less about frequency than about which mechanisms get written down: this
one was settled by removing it and seeing the effect go, which neither of the
others ever was.

The tolerances that existed to cover the slow state are down from 5.0 to 2.0, and
that is the point rather than tidiness. A tolerance wide enough to hold two states
is wide enough to hide a real regression, and this file was doing both: the shader
change found the same day cost 2.35 per cent on that row and its check passed on
the merits, failing only because the run happened to land slow.

**Check the board is quiet first, because it is not always.** `pgrep homescreen`
before a run, and a glance at `ps -eo pcpu,comm --sort=-pcpu | head`. A Flutter
embedder left running on the Pi 5 -- about a sixth of a core, drawing through the
same V3D, and not a service, so nothing restarts it and nothing announces it --
made a verification fail three times over on 2026-09-18, one run's GLES group
reading 57 and 71 per cent slow while its Vulkan group was perfect to four
tenths. That asymmetry is what it looks like: the two devices are measured in
sequence, so contention lands on whichever group was running. A group several per
cent slow together while the other device's group is clean is interference and not
a renderer.

**Run it from `/tmp` because that is where the numbers were recorded, not only
because it is convenient.** A byte-identical binary builds the stroked path in
0.238 ms from an ext4 filesystem and 0.283 from tmpfs -- eighteen per cent, on the
longest-running of the `recording` rows, while the shorter ones do not move at all.
Measured on an x86-64 desktop by copying one file between the two and hashing it
each time; consistent with how a text segment is mapped, and the mechanism is not
established from here. The board's baseline was recorded with the binary in `/tmp`,
as the block above puts it, so a run from a home directory or a mounted share is not
comparable with that file even at the same commit.

It cost an afternoon's confusion before it was found: the eighteen per cent was
first taken for the effect of moving the bench's frame definitions into their own
module, since the two measurements happened to come from two checkouts on two
filesystems. Two things settled it -- the binaries hashed the same, and copying each
one to the other's filesystem moved the number with the filesystem rather than with
the file.

`/tmp` is wiped on reboot, so a lockup costs the binary as well as the run.
A bench that dies instantly with `nohup: failed to run command './xtask'` is
that, not the board.

## What every fragment was paying for draws that never asked

Three fixes of one kind, found by pulling on a single unexplained row and
measured on the Pi 5 with three runs a side agreeing to a hundredth of a
millisecond. Each is work every fragment did that its own draw had not asked
for.

`shade` dispatched on the material kind with a chain of `if`s, each testing an
upper and a lower bound, so a fragment paid two float comparisons for every kind
ahead of its own -- and a solid color, kind zero and the commonest material
there is, matched none of the fourteen and fell through all of them to reach the
return at the bottom. It is a `switch` now with solid answered first.

`fs_main` called `blend_tint` unconditionally, to combine a vertex color with
the material. That function carries twenty-nine modes and the non-separable tail
behind them, and the mode a draw that asked for nothing gets is `Modulate` --
which is `src * dst`, one multiply. Written out at the call site, so the common
case does not enter the function at all.

`dithered` computed which kinds are dithered -- four comparisons -- before the
amplitude test that discards the answer for every draw that is not a gradient's
into a quantized target. The amplitude is tested first now.

| row | 2026-08-26 | now |
|---|---|---|
| Vulkan distance field, 1 sample | 13.363 | 9.225 |
| Vulkan tessellated, 4 samples | 15.976 | 4.523 |
| Vulkan tessellated, 1 sample | 13.661 | 3.694 |
| Vulkan full frame, mixed content | 21.199 | 14.623 |
| GLES distance field, 1 sample | 13.442 | 8.714 |
| GLES tessellated, 4 samples | 15.971 | 5.459 |
| GLES tessellated, 1 sample | 13.530 | 3.653 |
| GLES full frame, mixed content | 20.397 | 15.033 |

**Hash every scene before and after, and do not skip it.** No test in this tree
would have caught a mistake in any of the three: both backends run the same
WGSL, and the "software reference" the corpus compares against is lavapipe
running it too, so a shared error passes every comparison the suite makes. What
was done instead was to hash the pixels of all four hundred and twenty-six
catalog and corpus scenes on each side of each change. Zero differed, twice, and
that is what separates a dispatch cost from a shortcut.

**One wrong turn is worth keeping.** The first attempt at the dispatch reordered
the chain rather than replacing it: it recovered the rows it was aimed at and
made the mixed frame three per cent *worse*, because moving one kind up moves
every kind below it down. A switch removes that property rather than rebalancing
it.

The general lesson is about where this was visible from, which is nowhere except
the board. Not from a diff -- all three had been correct and unremarkable for as
long as they existed. Not from `cost.rs`, which counts passes, draws and
vertices and was right about all three. Not from the catalog, whose pixels do
not move. The chain had been there since the shader had kinds to dispatch on,
and the numbers it cost had been recorded as the baseline and read as the cost
of the work.

## A group's advanced blend, when the group does not fill the frame

`cargo xtask gate --software` is the command that runs what CI runs, and on this
machine it used to be unclean. Fourteen `blend/blend-mode-src-alpha-*` plates --
a group composited at half alpha with an advanced mode -- came out up to ninety
levels apart between llvmpipe's Vulkan and its GLES. CI does not see it: that
machine has llvmpipe from LLVM 20.1.2 and this one has 22.1.8, and the same
plates agree there.

**The lane is clean again as of 2026-10-05**, and the count had grown to
nineteen plates at up to 211 levels by then -- the image and atlas sources and
the clipped case joined the group family as Mesa moved. `catalog.rs` probes for
the defect rather than asserting through it: a group whose contents do not reach
its edge, composited under `Multiply`, has to match the same circle drawn
directly, and where it does not the twenty affected plates are skipped and named
in the gaps census. A device whose advanced blending is sound still compares all
four hundred and thirty-eight.

That probe cost a long re-derivation that this section would have saved, which is
worth saying here because this is the section that would have saved it: the
answer below was already written, already reduced to bare `ash`, and already
filed. Read it before investigating an advanced-blend disagreement.

Narrowed by varying one thing at a time, and the answer is not what any of the
obvious guesses said. Not the gradient behind the group, not whether the layer
was given bounds, not the mode, and not the size of the group's contents. It is
**where the group's contents sit inside it**:

| the group's contents | Vulkan | GLES |
|---|---|---|
| a rect covering the frame | composites | composites |
| a rect one pixel narrower, same origin | composites | composites |
| the same rect moved one pixel right | draws nothing at all | composites |
| a rect inset on all sides | draws nothing at all | composites |

Nothing about the recording changes across those four rows except the rect. Both
passes are the frame's size in all of them, the clears are the same, and the
compositing draw is vertex-for-vertex identical -- so what reaches the driver
differs only in the texels pass one leaves behind. Where it fails, the frame
comes back exactly as it was before the group.

It is the composite and not the mode. The same fifteen modes drawn *as draws*,
on a circle covering a third of the frame, agree between the two backends on
this machine -- that is the `blend/blend-mode-*` family, which the run that
found this compared and passed. What differs is compositing a layer's texture
with one of those modes when the layer's contents do not reach its edge.

What it is at the Vulkan level took five tries to say, and the first four were
wrong. Not a `renderArea` offset: this backend never sets one. Not the negative
viewport offset that crops a narrowed target. Not the *submission*: two
submissions with a fence between them fail exactly as one does. And not the
attachment's coverage either, which was the fourth answer and the one that got
filed -- partial coverage does fail, but only because of what it puts in one
particular place.

The fifth came from writing it out in bare `ash` with no renderer in it, which
is what should have happened before any of the other four were written down.
**The source is sampled once for the whole primitive rather than per fragment,
and every fragment is blended against that one value -- its color and its alpha
-- whatever the shader emitted there.**

That single sentence predicts every measurement taken across both
investigations, including the ones taken before there was a rule to predict
them. Destination `(0.264, 0.396, 0.616, 1.0)`, multiply:

| what the one source value is | predicted | measured |
|---|---|---|
| orange, alpha 1 | 60, 40, 24 | 60, 40, 23 |
| the intermediate's clear, alpha 1 | 7, 81, 47 | 7, 81, 47 |
| alpha 0 | 67, 101, 157 | 67, 101, 157 |
| orange, alpha 0.25 | 65, 86, 124 | 65, 86, 124 |

The three that pin it need no sampling at all, only a fragment shader whose
alpha varies. A rounded rectangle's coverage, alpha zero to one: not one pixel
of 1024 changes, including the interior ones where coverage is one and the
fragment is byte-identical to a constant-color shader's -- while the same
shader under ordinary alpha blending draws the antialiased shape, 572 pixels,
soft edges and all. Floor that alpha at a quarter so it never reaches zero and
the whole frame comes back one color, exactly the blend at a quarter, including
where the shader emits one. Turn the ramp inside out -- alpha one at the edges,
a quarter in the middle -- and the whole frame comes back at one. So it is one
fragment's value and not the minimum.

Everything else follows. A layer is cleared to transparent black, so a group
whose contents do not reach its edge has a transparent texel where the driver
looks, and the composite contributes nothing -- which is the table above. A
group whose contents *do* cover its target composites correctly even when its
alpha varies across it, which is measured and is what the rule requires. And
the fifteen catalog plates that were reporting a mode they never drew were
multisampled, which is not something a blend can see: four samples makes the
executor ask for antialiasing and draw the shape from its distance field, and a
distance field is exactly a fragment shader whose alpha varies. Single-sampled
they tessellate, and a tessellated shape's fragments all carry the paint's own
alpha.

Ruled out along the way, each by measurement rather than argument: the sample
count, the derivative -- stating the per-pixel rate as a constant instead of
taking `dpdx`/`dpdy` changes nothing -- a translucent source as such, since a
constant alpha of 0.92 blends correctly and matches the formula, the scissor,
the render area, a stencil attachment, a discarded multisample store, and the
submission.

Reported as gitlab.freedesktop.org/mesa/mesa/-/work_items/16243.

Which side is at fault the earlier drafts stopped short of saying, on the
grounds that a renderer accusing a driver had better be sure. The bare program
settles it: there is no renderer in it to be wrong. What was already true of
this side is still worth stating, since it is what made the reduction credible
-- the use of the extension is plain, `VK_EXT_blend_operation_advanced` named in
the pipeline's blend op, both operands declared premultiplied, `UNCORRELATED`
overlap, no framebuffer fetch and no barrier -- and the same recording is
correct on GLES, correct on the older llvmpipe, and correct against the
specification's own formula wherever it draws at all.
`every_advanced_mode_composites_a_group_by_its_equation` is what says the last
of those: fifteen modes at two layer alphas, thirty comparisons per device, all
within a unit.

That test exists because of this note. Nothing in the tree computed the expected
value for a group composite -- the blend-equation check pushes a single draw --
so there was no way to adjudicate. Now there is, and what is left is a question
about one driver version rather than about the renderer.

## A clear is rounded by the driver, and the drivers disagree

Two of them here put `[0.06, 0.07, 0.10]` on the screen as different colors.
Vulkan and GLES on this machine's hardware give 25 in the blue channel and the
software rasterizer gives 26, because a tenth of 255 is 25.5 exactly and nothing
says which way a half goes.

It is only the *clear*. The same tie in a fill does not do it -- a rectangle
filled with `0.5` comes back 128 on all three -- because a fill reaches the
target through the fragment stage and the fixed-function store, which round the
same way everywhere, while a clear is converted by the driver's own path. There
are a hundred and fifty-odd fills on a half across the two scene collections and
none of them costs anything.

What it cost was a unit on every pixel of ground a scene left uncovered, which
is inside the per-channel budget and is the reason it went unnoticed: the budget
is there for the arithmetic of drawing, and it was being spent on the color the
frame started at. On a pale ground the same mistake is louder -- nine tenths of
255 is 229.5, and a shadow scene written that way differed from GLES on 79 per
cent of the frame, all of it background.

State a ground in eighths of a byte and there is nothing to round.
`no_scene_clears_to_a_color_on_a_rounding_tie` checks both collections.

## Advanced blending on this machine's Vulkan, in two disguises

Two arrangements answer an advanced blend with an empty frame on lavapipe, and
GLES -- the same Mesa through a different extension -- is correct for both.
Neither is this renderer: the pipeline state is the same either way and the
pictures agree on GLES.

They were written down here as two defects and they are one. The section above
has the rule and how it was reduced; what belongs here is what each looked like
before there was a rule, because that is what a reader will meet first.

The first was filed as "any advanced blend under multisampling". A sample count
is not something a blend can see. What four samples changes is the *route*: the
executor asks for antialiasing and the shape is drawn from its distance field
rather than tessellated, and a distance field is a fragment shader whose alpha
varies. So the claim was naming the switch that selected the failing
arrangement. Fifteen catalog plates were reporting a mode they never drew, and
they are single-sampled now, which costs them nothing -- their subject is what a
blend computes rather than where an edge falls. That fix still holds and now for
a stated reason: a tessellated shape's fragments all carry the paint's own
alpha.

The second is a *group* whose contents do not reach the edge of its own target,
composited with an advanced mode -- which is every group a plate is likely to
draw. A layer is cleared to transparent black, so such a group has a transparent
texel where the driver takes its one source value, and the composite contributes
nothing. It fails at any alpha and any sample count, where the same mode on an
ordinary draw works and the same group filling its target composites correctly.
There is no workaround here -- the composite is what a group is -- so the sweep
that asks whether a plate can show its mode asks every device and passes if any
can.

Both were found by writing the plate and looking at the output, and neither is
visible from a diff, from `cost.rs`, or from the cross-backend comparison, which
skips these scenes because the *preferred* Vulkan device has no advanced-blend
extension at all.

## Saying when it was last checked

`cargo xtask gate` prints one line about the timing baseline, beside the skip
census and read the same way: how many commits have touched what the bench times
since a board run last passed. It is not a threshold and it cannot fail; timing
needs a quiet machine, and the one the gate runs on spreads its own medians by
up to half.

The count is from a line in the baseline itself -- `# Last checked against the
board:` and a commit -- rather than from when the file last changed. The
difference matters, because the ordinary outcome of a check is that the numbers
*pass* and are not re-recorded, and keying on the file would count those as
drift. Update the line when a `--check` run passes, whether or not you re-record.
A test asserts the line is there and parses, since nothing else would notice a
comment in a data file going missing, and a baseline that has stopped saying
when it was checked reports "current" forever.

### Which commit it names, and the way that went wrong

A hash cannot be recorded inside the object it names, so the line always names a
commit older than the one writing it -- and choosing which older one is where
this went wrong once, in a way worth keeping because the failure looked like a
correction.

The line had named the commit before the one whose numbers were recorded, so the
gate read one commit of drift where the answer was none. That was fixed by moving
the line forward one commit. The reasoning was right and the commit it landed on
was not: the commit it moved to *changes what the bench times*, and says so in
its own message -- "the distance-field benchmark came back four per cent faster
on the Pi 5". So the line came to name a state no board run had ever passed
against, and the drift count started from the wrong place.

Nothing could see it. A `--check` was not run at the moment the line moved,
because moving it was a documentation fix; and when one was run eight commits
later it failed, on a row that had nothing to do with any of those eight.

The rule that comes out of it: **the line may only name a commit that touches
nothing the bench times, or the commit whose state a run actually passed
against.** A commit that only edits the baseline file qualifies, so the ordinary
case -- re-record and name the commit you measured, in the same commit -- is
both legal and the shortest path. Moving the line onto a renderer commit is what
is not allowed, however plausible the arithmetic looks.

### What that hid

Finding it needed three commits benched, three runs each, in one sitting on one
board -- the commit the numbers came from, the commit the line had been moved
to, and the tip. The first reproduced the recorded numbers to within four tenths
of a percent and passed. The tip was level with the middle one everywhere, the
largest gap between them a fifth of a percent. So all of the movement belonged
to the middle commit, which drew a stroke as the difference of two offset shapes:

| row | before | after | |
|---|---|---|---|
| vulkan distance field | 9.225 | 8.857 | -4.0%, which its message claimed |
| gles distance field | 8.714 | 8.919 | +2.4%, which nobody noticed |
| vulkan full frame | 14.623 | 14.442 | -1.2% |
| gles full frame | 15.033 | 14.530 | -3.3% |

The four tessellated rows do not move at all, which is the check on that
attribution: the change is to the analytic stroke and those rows never take it.

The commit is a net win on this board and one row of it is a loss. Both
full-frame numbers improve, and a frame of mixed content is what an interface
pays; the GLES analytic-stroke row costs two and a half per cent for it. Worth
recording rather than averaging away, because the two backends run the same
WGSL: a change that moves them in opposite directions is a fact about the two
compilers, and the next such change wants this one to have been written down.

It went unseen because of what sits beside it. The Vulkan row it is paired with
carries a five per cent tolerance of its own, for a bimodality documented in the
baseline's header -- so the run that would have failed on GLES was the only one
that could have said anything, and it was never made.

### A function nothing called, compiled by every driver

The same measurement has a corollary nobody had drawn from it. A shader is not
a program with a linker that drops what nothing reaches: naga emits an uncalled
function into the GLSL exactly as it emits a used one, so dead code in a shader
source is code every driver parses and compiles for the life of the program.

`solid.wgsl` had one. `outline_if_asked` went in with the commit that traced an
outline from the same distance field, was never called by anything, and stayed
for every commit after -- two hundred and seven bytes of GLSL and seventy-eight
words of SPIR-V. Whether that is measurable is not established and this document
will not guess; what is established, in the section below, is that this shader's
cost is a step function of its size on this board, and that is enough reason not
to carry code nothing runs.

`every_function_in_a_shader_is_either_called_or_a_stage` in
`crates/emblema-shaders/tests/sources.rs` is what would have said so. It reads
the WGSL rather than the generated output, since a call graph is legible there
and not in the GLSL, and it catches a chain one link at a time: a helper called
only from a dead function still reads as called, so removing the root is what
names the next one.

### And a smaller shader is not a faster one

The corollary above says not to carry code nothing runs. It does not say that
removing code makes anything quicker, and the same board says plainly that it
does not.

Two commits shrank `solid.wgsl` in one afternoon. `1f64a28` folded a rounded
rectangle's fill and its outline into one expression, which took 431 bytes of
GLSL and 68 SPIR-V words out; `e9833c8` deleted the uncalled function above,
another 207 bytes and 78 words. Benched three commits over sixteen runs, four a
side in the faster of the two Vulkan states:

| row | before | after | |
|---|---|---|---|
| vulkan full frame | 14.444 | 13.955 | -3.4% |
| vulkan tessellated x4 | 4.481 | 4.522 | +0.9% |
| vulkan tessellated x1 | 3.661 | 3.691 | +0.8% |
| gles full frame | 14.540 | 14.835 | +2.0% |

All of it is `1f64a28`. The deletion measures at nothing at all -- four runs of
the tip against four of `1f64a28` alone are indistinguishable on every row --
which is the answer to the question that commit deliberately left open, and is
what should happen if a driver drops unreachable code from the program it
actually compiles even though naga emits it into the source.

Three things worth keeping from the rest of it. The change went *four* ways at
once, not one: a frame three per cent cheaper on one backend and two per cent
dearer on the other, from the same source. Neither distance-field row moved,
and that is the path the folded function serves -- so what the other four rows
responded to is not the arithmetic that changed but the size and shape of the
program around it. And the direction is not predictable from the size: a
strictly smaller shader made the row that matters most faster on Vulkan and
slower on GLES.

The lesson is the one above it, with the sign removed. Shader size is a step
function on this board, the steps are not all downhill, and the only way to know
which way one goes is to run it.

### The two per cent is not lying around to be picked up

The obvious next question is whether the GLES row can have its two per cent back
without Vulkan giving up its three. Four shapes of the same function were
benched on the board, three runs each, all in one sitting, and the answer is no
-- at least not from rewriting this function.

| the function's shape | vulkan frame | gles frame |
|---|---|---|
| folded, as it is now | 13.95 | 14.83 |
| the fill given its own call, tail still shared | 13.86 | **14.98** |
| the redundant `select` deleted | 13.93 | **14.96** |
| two paths and an early return, as before the fold | **14.55** | 14.49 |

The middle two are the interesting rows. Both are perfectly reasonable
rewrites, one splits the shape in two and one merges it further, and *both* make
the GLES frame worse than what is there now. So the current shape is not merely
the one nobody has looked past -- among the shapes tried it is a local minimum
on that row, and the two per cent is not slack waiting to be reclaimed.

The last row is the trade stated plainly: put the function back the way it was
and GLES returns to 14.49, better than the 14.54 it had, while Vulkan goes to
14.55 from 13.95. It is the same change read from either end. Nothing here
splits the difference.

Two things are worth carrying out of it. The GLES *distance-field* row does not
move across any of the four -- 8.91, 8.92, 8.83, 8.92 -- so this function's own
cost is not what is being measured on the frame row at all; what moves is the
rest of the program compiled around it. And the `select` really is redundant,
since `outer_radius` already carries the clamped radius wherever the stroke is
zero. Deleting it costs a per cent of the GLES frame, which is a strange price
for removing something that does nothing, and is the clearest statement in this
file of how little the generated source predicts.

## What a shader costs on this board, measured the hard way

The baseline went sixteen renderer commits unchecked, and checking it found
every row between five and eleven per cent slower. The board had not changed:
the baseline commit, cross-built and run the same afternoon, reproduced its own
numbers to a tenth of a tenth of a per cent -- 13.362 against 13.363 -- which is
what makes the rest of this a statement about the code.

Bisected on the board, one cross-build and one run per step, reading the
tessellated single-sample row:

| commit | ms | against the baseline |
|---|---|---|
| the baseline | 13.661 | — |
| a nine-patch in one draw | 13.663 | +0.0% |
| a point field in one draw | 14.148 | +3.6% |
| a blend as a color filter | 14.713 | +7.7% |
| a gradient at a mesh's coordinates | 15.125 | +10.7% |

Three shader changes, each individually reasonable, each about three points.
None of them added work to the path being measured: a tessellated rectangle
filled with a solid color takes no gradient, no point field and no color filter.
What they added was *size* -- another material kind, another branch in the
filter tail, another coordinate -- to a shader every draw compiles.

One of the three was also a plain mistake, and fixing it recovered all of them.
`select` in this language evaluates both of its operands, so choosing between two
calls to the gradient mapping ran it twice per fragment; and it had been hoisted
above the branch chain, so a solid fill ran it twice as well. Selecting the
*input* and calling the mapping once, from inside a gradient's arm, put the
tessellated rows at 1.6 to 1.7 per cent *below* the baseline.

That the last three points of a ten-point regression were worth twelve is the
part to remember. The cost of a fragment shader here is not the sum of what its
branches do; it is a step function of what the whole thing needs at once, and
the compiler's register budget is the step. So a change that adds nothing to the
measured path can still cost three per cent, and a change that removes a little
can recover much more. Neither is visible from a diff, and neither is visible
from `cost.rs`, which counts passes, draws and vertices and is right about all
three.

**Measure through GLES on this board. One Vulkan configuration will not hold
still, and it is the one worth measuring.** Ten runs of a binary gave a GLES
distance-field figure spread over 0.017 ms and a Vulkan one that jumped between
13.35 and 13.82 — same process, same GPU, printed minutes apart in the same
run, and only one of them moving. So it is not heat, not the clock (V3D sat at
960 MHz throughout) and not the board.

Narrower than that: within one run the other two Vulkan configurations are
steadier than the GLES row. Over six runs the tessellated single-sampled figure
spread 0.008 ms and the four-sample one 0.05, while the distance field beside
them spread 0.277. What separates that configuration is draw count — an
analytic shape carries its geometry inside its material and cannot merge, so it
is a hundred and sixty draws where the tessellated ones are a single merged
draw. The variability is therefore in something the Vulkan backend does per
draw rather than per frame, and it does it differently from one process to the
next.

It scales with the draws, which is the next thing that was worth measuring
rather than assuming. Tripling the shape count from a hundred and sixty to four
hundred and eighty took the gap from 0.45 ms to 1.06 -- about two and a half
microseconds per draw either way. So the two states differ in what a draw
costs, not in a fixed charge per frame.

One candidate is ruled out and it is the obvious one. Every submission
allocates, fills and frees its buffers, and the analytic configuration's
materials buffer is forty kilobytes against the tessellated one's two hundred
and fifty-six — so host-visible bytes written looks like the culprit until the
geometry is counted. The analytic frame carries 640 vertices and 960 indices;
the tessellated frame carries 3840 and 10560. The *stable* configuration writes
several times more host-visible memory per frame than the unstable one, so the
quantity written is not what varies.

What is left is per-draw work: a hundred and sixty descriptor rebinds, pipeline
lookups and draw calls against one of each. Hashing is not enough to explain it
— two `HashMap` lookups a draw at tens of nanoseconds against a gap of two and
a half *micro*seconds a draw.

Memory placement is not it either, which was the next guess and is now ruled
out. Printing what `gpu-allocator` chose for a submission's first host-visible
buffer gave the same answer in every run — `DEVICE_LOCAL | HOST_VISIBLE |
HOST_COHERENT`, offset 8294656, size 23040, byte for byte — across runs that
came out fast and one that came out slow. Same memory type, same offset, same
size, different speed.

So: not the quantity written, not where it was written, not the hashing. What
remains is the driver's own cost of a draw — and which half of that, recording
or executing, turns out to be answerable without `perf`, which is not installed
here anyway. Timing the two phases separately inside the backend, around the
`vkCmd*` loop and around the submit-and-wait, separates them cleanly. Over
thirty-one runs, per frame at a hundred and sixty draws:

| | recording | submit and wait | frame |
|---|---|---|---|
| fast, 26 runs | 0.66 ms | 12.66 ms | 13.36 ms |
| slow, 5 runs | 1.01 ms | 12.70 ms | 13.81 ms |

**The gap is in recording commands, not in running them.** Recording separates
the two states by 57 percent; submit-and-wait, which contains all of the GPU's
work, differs by 0.3 and accounts for a twelfth of the gap. Every slow frame
had a slow recording phase and no fast frame did — the correlation is exact
across all thirty-one. The extra 0.35 ms over a hundred and sixty draws is 2.2
microseconds each, which is the same per-draw figure the shape-count sweep
above arrived at from the outside.

The GLES device is the control and reads zero on both counters, since they
count only what the Vulkan backend does.

Two more per-process candidates are ruled out, both of them things fixed when a
process starts. Pinned to one core with `taskset -c 2`, one run in ten still
came out slow; with address-space randomization off under `setarch -R`, two in
ten did. So it is neither which core the recording runs on nor where the
driver's code and buffers land in the address space.

What is left is narrow: something the V3D driver decides once per process that
changes how expensive it is to *write* a command, with the commands themselves
costing the same to execute. A memory type for the command pool that is
uncached on one path and cached on the other would have exactly this shape, and
that is invisible from this side of the API — settling it wants Mesa
instrumentation. But the search space is now half its size, and the GPU, the
scheduler and the board's thermals are all out of it.

That is a diagnosis of where, not yet of what. Until it is both, establish a
difference on the GLES row, where a three-run cluster is tight to a couple of
hundredths.

**One run is not a measurement: the Vulkan figure lands in one of two speeds.**
Five consecutive runs of one binary, on a cool fanned board minutes after a
power cycle, came back 13.374, 13.820, 13.368, 13.809 and 13.805 ms. Not a
spread — two clusters, 13.37 and 13.81, each internally tight to a few
hundredths, 0.44 ms apart. Which one a run gets appears to be settled when the
process starts and holds for its whole two hundred frames, which is why every
individual run looks impeccable: its own p99 sits within 0.06 ms of its median
and says nothing.

Three percent is the gap, and it is the same size as differences worth
reporting, so a one-run-each comparison can invent one or hide one.

What it is not: heat, and not accumulated session state. It was read as heat
first, because the afternoon's slow numbers followed hours of running at 77 to
85 degrees. Then a fan brought idle to 67 and the number did not move; then a
power cycle brought it to 61 on a fresh boot and it still did not. Two
plausible mechanisms, tested, both wrong. `vcgencmd get_throttled` is still
worth reading after a run and still latches, but a clean reading does not make
two runs comparable.

The rule that follows does not depend on knowing the cause: **alternate the two
builds in one sitting and take at least three runs of each, then check the
clusters do not overlap.** A difference established that way survives whatever
this is. The three percent an uber-shader charges for one more material kind
was measured so: three runs of each build, 13.376/13.419/13.385 against
12.960/13.019/13.006, neither side straying into the other's band, and it
reproduced in a later session. A single run either side would have proved
nothing at that size.

A whole-frame figure seems less sensitive to the drift than the micro-benchmark
paths -- 21.2 ms held across it -- but that is an observation rather than
something to rely on.

## Why the timing baseline lives here and not on the workstation

Asked directly, because "the desktop is too noisy" had been an impression
rather than a number. Three runs of `xtask bench --skip llvmpipe`, release
build, on the development machine — a sixteen-core Ryzen with an integrated
Radeon and a desktop session running — and the spread of the *median* across
those three runs, per row:

| row | min | max | spread |
| --- | --- | --- | --- |
| vulkan tessellated, 1 sample | 0.743 | 0.749 | 0.8% |
| vulkan distance field | 0.804 | 0.818 | 1.7% |
| gles distance field | 0.913 | 0.932 | 2.1% |
| gles tessellated, 1 sample | 0.791 | 0.813 | 2.8% |
| gles tessellated, 4 samples | 1.523 | 1.591 | 4.5% |
| gles full frame | 1.997 | 2.139 | 7.1% |
| vulkan full frame | 2.002 | 2.383 | 19.0% |
| vulkan tessellated, 4 samples | 1.270 | 1.927 | **51.7%** |

Three per cent is the size of a difference worth reporting. A tolerance loose
enough to admit the bottom row would admit anything, and one tight enough to
mean something would fire on half the runs. That is the whole answer: this
machine cannot gate a timing baseline, and the reason is not the tail but the
median — two of eight rows move by more than any regression this project would
be trying to catch.

The tails are worse and are worth seeing once. In these three runs a row with a
median of 2.068 ms reported a ninety-ninth percentile of **374.752 ms**, and
another with a median of 0.748 ms reported 63.432 ms. Those are this process
being descheduled, not a frame taking that long, which is what the bench's own
preamble warns about.

Against that, the Pi 5 on the same day: seven of eight rows within 0.3% of a
recorded baseline across two runs, and the four GLES rows within a tenth of a
per cent. That is five hundred times steadier on the rows that matter, and it
is why the recorded baseline is a board's and why `--check` is run there.

Scoped honestly: this is a statement about *this machine as configured*, with a
compositor competing for the same GPU. A quiet, headless x86 runner might do
better and has not been tried. What is settled is that the workstation someone
is working on is not that machine, and that the cheap version of a perf gate —
record a baseline here, check it in CI — cannot work.

The gate that does work on every commit is a different quantity entirely: see
`crates/emblema-testkit/tests/cost.rs`, which counts what a frame does rather
than timing it.

## A second board, and what it says about reading a green run

A Radxa Zero 3 (RK3566, Mali-G52, Debian 12) is the other target here, and
almost everything above needs adjusting for it. Its name resolves as `.local`
and not `.lan`. It has no `rsync`, so the sysroot comes over `tar` piped
through `ssh` instead. And it needs its *own* sysroot: glibc 2.36 against the
Pi's newer one, so Pi-built binaries will not run there, while binaries built
against this one run on both. Its gcc is 12, so the last link argument ends
`/12` rather than `/14`.

Take the binary paths from `cargo test --no-run --message-format=json`,
filtering for `.profile.test == true` and reading `.executable`. Not from
`ls -t`: a stale binary from an earlier build otherwise gets shipped, and reads
as the change not having worked.

**Vulkan does not reach the Mali GPU there, and a run will not say so.** The
loader lists a `panfrost_icd.json`, and asking for Vulkan with only that ICD
fails inside `enumerate_physical_devices` with `ERROR_INITIALIZATION_FAILED`.
Leave the other ICDs in place and Vulkan succeeds -- on **llvmpipe**, which is
also installed, so a Vulkan suite runs to completion on a software rasterizer
while a GPU sits beside it unused. The backend that does reach the Mali part is
GLES. On it the public API suite is 228 passed, 0 failed.

**Two failures turned up there, and this paragraph got both of them wrong the
first time.** It said Debian 12's llvmpipe was "old enough to be wrong" and that
neither failure was this renderer's. One of those is half right and the other is
backwards, and both were settled by CI rather than by a board.

The clip one is a driver defect, and not an age. An antialiased line writes half
coverage one pixel outside a rectangular clip -- `(55, 87)` where the scissor
begins at 56 -- on llvmpipe at Mesa 15.0.6 and again at Mesa 25.2.8, which is
current. Mesa 26.1.7, RADV and PanVK are clean. The scissor this renderer
records is right either way, and identical whether the paint asks for
antialiasing or not; what differs is that antialiasing opens a multisampled
pass. Half coverage is two of four sample positions, which reads as a per-sample
scissor test half a pixel out. `public_api` probes for it now and skips the clip
half by name where it finds it.

The dithering one was ours. The test compared a dithered eight-bit render
against the same gradient in a half-float target, and a half-float's step near
six tenths is larger than the error being measured -- so it was measuring its
own reference, and its threshold had been fitted to whatever that came to on one
device. Calling it a driver defect was the comfortable reading and the wrong one.

The moral is not about llvmpipe. **A board tells you a device disagrees; it
cannot tell you who is wrong.** Both of these needed a third device and a fourth
driver before the answer was clear, and one of them needed the answer to be
"us".

The other lesson from that board. **`test result: ok` is not a result.** A suite here reported three
passing tests in a quarter of a second having rendered nothing, because the
context it wanted could not be created and the test returned early. Read the
skip lines. `cargo xtask verify` counts them for you on a workstation; running
bare binaries on a board loses that, so grep for `skipping` alongside
`test result`, and read the "drew N of M" line the catalog prints.

**Grepping for them needs `--nocapture`, and forgetting it looks like success.**
A skip is an `eprintln!` inside a test that then passes, and the harness holds
the output of a passing test. Without the flag the skips are not in what you
grep, so the count comes back zero -- which is indistinguishable from a run
that skipped nothing, and is the more reassuring of the two readings. A full
run on the Pi 5 read as zero skips that way and has a hundred and twenty-one.

Count them with `grep -c skipping` and not by adding up a `uniq -c` by eye. The
first number written here was a hundred and fifteen and was wrong twice over:
the categories were mis-added, and the pattern had a colon in it, which four
lines saying "skipping validation assertions" do not.

Most of those say the validation layer is unavailable,
which is a statement about the board rather than about the renderer: the layer
is not packaged there, so the API use those tests make goes unchecked while
their pixels are still compared. The rest are capability gaps that name
themselves -- no device offering advanced blending, a swapchain returning one
image for every acquisition, a C shared library not built beside its test.

## What it found

Recorded because the point of the exercise is not the procedure. Every one of
these was invisible on a workstation, and each was a defect rather than a
tolerance that needed widening.

- **A display controller was chosen without asking which CRTCs a plane can
  drive.** `possible_crtcs` was not consulted, so plane and CRTC were paired by
  order. Pi 5 HDMI went from one mode in five to five in five.
- **The outlier budget had never worked.** `Tolerance::outlier_fraction` is
  documented as the fraction of pixels allowed to exceed the per-channel bound,
  and was counted as the fraction differing at all — so ordinary rounding was
  charged to it, and rounding is what the per-channel bound exists to absorb.
  The board is where it showed: four pixels of sixteen thousand a sample apart
  on a circle's edge, against a budget of sixteen, rejected on the 148 pixels
  that were a single level out.
- **The per-channel bound counted draws rather than stores.** A group is
  rendered into a target of its own and composited out of it, so its fragments
  are quantized twice — two devices whose arithmetic differs in the last bits
  land two levels apart, on interior pixels rather than edges.
- **An aliased edge can fall either side of a pixel center.** Neither
  specification requires two implementations to compute the same matrix-vector
  product to the last bit, and a fused multiply-add is enough to move an edge by
  a hair. Without multisampling there is no partial coverage to soften it, so
  the pixel goes one way here and the other there at full scale.

The tiler also answered an open design question that a desktop had been giving
the wrong answer to: four samples cost 1.17× on V3D against 1.78× on a desktop
GPU, which closes the margin the architecture had been reasoning about.
`docs/architecture.md` carries the measurement.

**The KMS lane runs on a real display controller, not only on VKMS.** All
forty-six tests in `emblema-present-drm` pass on the Pi 5 -- nineteen unit,
five over the `IN_FORMATS` parser, twelve against the scanout stand-in, and the
ten that take DRM master and commit a frame. Measured 2026-10-06 at 9dc1a2e,
on an idle board at 64.8 C.

These are the run rather than the tree, so nothing updates them when a test is
added: a later number means the board was run again. The figure stood at
thirty-four for a while and was wrong in the understating direction even then,
because the count left `in_formats.rs` out entirely -- which is the direction
that reads as a gap someone might set out to fill. The board
has two display controllers, `vc4` driving HDMI and `drm-rp1-dsi` driving the
panel, and a separate `v3d` render node, which is the split render/display
topology `architecture.md` says VKMS stands in for. It is now checked against
the thing itself rather than only against the stand-in.

They need no display server running to get master, and there is none on this
board. Note that `cargo test --workspace` does *not* build them -- the crate is
reached through the facade's `drm` feature -- so a cross-compiled suite has to
ask for `-p emblema-present-drm` by name or silently leave the whole crate
behind. Mine did, on the first run: the total was 809 where it should have been
834, a gap of twenty-five, which is what the crate held that day.

## What a second full run found, and why none of it was the renderer

The findings above were defects. A full run on 2026-09-22, after the crates were
published, found four failures and **not one of them was in the renderer.** Each
was a test claiming more than it could know, and the four together say something
the individual entries do not: a suite whose only two devices are software
rasterizers cannot tell its own assumptions from the code's behavior. Four
separate assertions had quietly recorded lavapipe's arithmetic as an invariant.

- **A coverage bound was measuring a driver's guard band.** A stroke `1e30` wide
  was required to cover more than a quarter of the frame. The vertices reaching
  the device are finite and enormous -- in clip space the largest is 7.8e27,
  against a unit cube -- with no NaN and no infinity, which is the part this
  renderer decides. lavapipe clips such a triangle in float and covers about half
  the frame; v3d bins into a bounded fixed-point format, cannot express the
  coordinate, and drops the primitives for a coverage of exactly zero. Both are
  defensible, and `1e9` covers the frame on both at 7.8e6. Any coverage assertion
  above `MAX_COORDINATE` is an assertion about a rasterizer.
- **A texture filter had no term in the tolerance.** Four corpus scenes diverged,
  every one at a maximum delta of exactly three and every one sampling a texture.
  Established by reduction: one scene, an eight-texel sheet magnified about
  fourteen times, is byte for byte identical across the two devices through
  `Sampling::Nearest` and reaches three across seventy-two per cent of the frame
  through `Sampling::Linear`. Nothing else changed. This is the fourth time a
  board run has found the tolerance model wrong in a new way, after the outlier
  budget, the per-store bound and the tie budget above -- which is worth reading
  as a pattern rather than as four accidents.
- **Two orders of one operator were required to agree bit for bit.** A channel
  swap and a blur commute exactly in real arithmetic; where the rounding falls
  does not, since one order quantizes the swapped color before the weighted sum
  and the other quantizes the sum before the swap. One level on a hundred and
  twenty-seven pixels, reported as a dropped filter.
- **A capability was confused with a result.** A plate no device could render and
  a plate every device rendered identically both arrived as one `false`. No device
  on the board has advanced blending -- v3d has not got the extension, the GLES
  context is the same v3d, and that board's llvmpipe reports it absent where the
  desktop's newer one has it -- so nothing was asked, and the message said the
  mode was not reaching the picture. The skip written for exactly that machine
  could never run, because the per-plate assertion reached it first.

The shape to take from it: a cross-device assertion states a property of this
renderer or a property of a rasterizer, and the two are easy to write down in the
same sentence. A board is the only thing on this bench that tells them apart.

## Measuring pacing there, and what a number will have to say

The suite says the frame loop works. It does not say the display kept up, and a frame
rate cannot: sixty frames a second to a sixty-hertz panel and sixty to a
hundred-and-twenty-hertz one read the same on a counter. `docs/architecture.md` has
the mechanism -- blanks counted from the kernel's flip sequence, not intervals timed
in userspace -- and this is the recipe and the preconditions.

The number comes from the panel example, **built release**, because the suite here is
cross-compiled without it and a debug frame against a real sixteen-millisecond budget
measures the optimizer:

```sh
cargo build -p emblema-present-drm --release --example panel \
  --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/examples/panel "$PI:/tmp/"
ssh "$PI" 'cd /tmp && EMBLEMA_DRM_CARD=/dev/dri/card0 SECONDS=30 ./panel'
```

`/tmp` for the reason the bench rows want it, `card0` because that is `vc4` and the
one that scans out what this renderer exports -- the crate documentation has the
table, and `cargo xtask drm` says what a given machine could host.

A figure taken here has to carry what the timing baseline's header carries, and for
the same reason: the governor pinned to `performance`, `vcgencmd get_throttled` clean
before and after, nothing else on the board, three runs rather than one, and the
commit it was taken at. Two more belong to this measurement rather than to the bench:
the **ring depth**, because a deeper ring hides a slow frame instead of missing a
blank, and the **CPU-wait count**, because a run with no misses and a wait on every
frame is a pipeline carrying a frame of latency rather than one that is keeping up.

Read a zero carefully. It means no frame was late *enough* to miss a blank at that
ring depth, which is not the same as headroom -- and calibrating the example's stall
probe showed the difference is a whole frame period wide. Two things say there was
headroom: the offscreen figure beside it, which is what `cargo xtask bench` measures,
and the same run at `DEPTH=2`, where there is no spare buffer for a late frame to hide
behind.

### What it did, measured 2026-09-22 at 251e1e0

Both display controllers, three runs of ten seconds each, governor pinned, board at
61 C with `vcgencmd get_throttled` clean, nothing else running and no display server,
release build run from `/tmp`. Ring depth three, the scene's default three cards.

| controller | mode | period | frames | flips / blanks | missed | cpu waits |
|---|---|---|---|---|---|---|
| `vc4`, HDMI | 1280x1440 at 60 Hz | 16.668 ms | 600, 599, 599 | 594/593, 593/592, 593/592 | 0, 0, 0 | 1, 1, 1 |
| `rp1-dsi`, DSI | 800x1280 at 60 Hz | 16.645 ms | 600, 600, 600 | 594/593 each | 0, 0, 0 | 1, 1, 1 |

**Every vertical blank was latched, on both controllers, in all six runs.** Sixty
frames a second at 1280x1440 and at 800x1280, with one CPU wait apiece -- the
modesetting commit, and nothing after it. The kernel's timestamps agree with the
blanks counted to a millisecond over ten seconds in every run.

Three things that number says, and one it does not.

It says the **fence-on-commit path works on hardware at scale**. One CPU wait in six
hundred frames means five hundred and ninety-nine commits handed their fence to the
kernel and every one of them latched. The eight-frame test asserted that already; this
is the same claim three orders of magnitude further along, on two different
controllers.

It says the **counter would have noticed**. Under `STALL=10` on the same controller
and the same pinned board, five hundred and forty-six frames produced fifty-four
stalls and fifty-four missed blanks. A zero from this measurement is "nothing was late
enough", not "nothing is counting", and that distinction was measured rather than
assumed.

It says the **rounded refresh would have been the wrong period for both**. HDMI's mode
is 59.995 Hz and DSI's is 60.08, and both report "60.00" to the whole-hertz figure the
wait budget uses -- in opposite directions. `exact_frame_nanos` is why the
cross-check above lands to a millisecond rather than to a few.

What it does **not** say on its own is that there was headroom. At ring depth three a
frame taking nearly the whole period and one taking a tenth of it both miss nothing,
so this figure and `cargo xtask bench`'s offscreen rows answer different questions and
neither substitutes for the other. The run below is what says it.

### What depth two said, measured 2026-09-23 at f190736

A two-deep ring has nowhere to hold a finished buffer back, so a frame that overran
its period misses a blank instead of being absorbed. The same scene at depth two is
therefore the stronger claim, and it was run the same way: three runs of ten seconds
on each controller with a depth-three control in the same session, governor pinned,
board from 62.0 C to 65.3 C with `vcgencmd get_throttled` clean throughout, load
average 0.00 before, no display server, release build from `/tmp`.

| controller | depth | frames | flips / blanks | missed | cpu waits |
|---|---|---|---|---|---|
| `vc4`, HDMI | 2 | 600, 600, 600 | 594/593 each | 0, 0, 0 | 1, 1, 1 |
| `vc4`, HDMI | 3 | 600 | 594/593 | 0 | 1 |
| `rp1-dsi`, DSI | 2 | 599, 599, 600 | 593/592, 593/592, 594/593 | 0, 0, 0 | 1, 1, 1 |
| `rp1-dsi`, DSI | 3 | 600 | 594/593 | 0 | 1 |

**Zero at depth two, on both controllers, in all six runs.** So the ring was not
covering for anything: the renderer fits a frame inside the period with no spare
buffer to hide behind, and the sixty-a-second figure above is a statement about the
renderer rather than about the ring.

That is only worth reading if the depth axis does anything, which it does. Scaling the
scene with `CARDS` on HDMI, ten seconds per point, same session:

| cards | depth 2 | depth 3 |
|---|---|---|
| 3 (default) | 0 missed, 59.9 fps | 0 missed, 60.0 fps |
| 12 | 294 missed, 30.1 fps | 111 missed, 48.7 fps |
| 24 | 294 missed, 30.0 fps | 242 missed, 35.3 fps |
| 48 | 388 missed, 20.0 fps | 363 missed, 22.8 fps |
| 3, `STALL=10` | 86 missed | 54 missed |

Two things to read off it. The deeper ring is worth a great deal once the scene is
over budget -- at twelve cards it turns 294 missed blanks into 111 -- which is what
says the two-deep result above was not measuring an inert knob. And the depth-two
rates are clean submultiples of sixty where the depth-three rates are not: with no
spare buffer a late frame waits a whole period, so the loop locks to 60/n, while at
depth three rendering overlaps scanout and lands in between. That is the mechanism
`target::DrmScanoutTarget` describes, observed rather than inferred.

The headroom is real and it is not large. At depth two the scene still misses nothing
at **four** cards and misses 119 blanks at five, so what the published figure has in
hand is more than one card's worth of work and less than two. A frame budget is not
the same as a frame rate, and this is the frame budget.

## A third board, and two tile GPUs disagreeing

A StarFive VisionFive 2 -- JH7110, four SiFive U74 cores, Imagination PowerVR
B-Series BXE-4-32 -- answers a question the other two boards could not. V3D and
Mali are both tile architectures, so a conclusion drawn from one and confirmed on
the other reads as a fact about tile GPUs. PowerVR is the third, and it is the
oldest and most committed of the tile designs: deferred, with hidden-surface
removal. If a conclusion is about tiling rather than about V3D, it should hold
here.

One does and one does not.

### Getting a binary onto it, which the Pi's recipe does not do

The sysroot instructions at the top of this file assume the board can supply one.
This image cannot: it is Ubuntu 24.04 with no toolchain, so
`/usr/lib/riscv64-linux-gnu` holds `libc.so.6` and no `libc.so`, no `Scrt1.o`,
and there is no `/usr/lib/gcc/riscv64-linux-gnu` at all. Nothing to rsync. The
host's `riscv64-linux-gnu-gcc` is no better off -- `-print-file-name=Scrt1.o`
echoes the name back, which is how that compiler says it has no C library.

What worked, and is the recipe for any board whose image carries no development
packages: read the board's glibc version, then assemble the sysroot on the host
from the distribution's own packages for that exact version.

```sh
S=$HOME/.cache/starfive-sysroot
P=http://ports.ubuntu.com/ubuntu-ports/pool/main/g/glibc
# `ldd --version` on the board said 2.39-0ubuntu8.9; match it rather than approximate.
curl -fsSLO --output-dir /tmp "$P/libc6_2.39-0ubuntu8.9_riscv64.deb"
curl -fsSLO --output-dir /tmp "$P/libc6-dev_2.39-0ubuntu8.9_riscv64.deb"
dpkg-deb -x /tmp/libc6_2.39-0ubuntu8.9_riscv64.deb "$S"
dpkg-deb -x /tmp/libc6-dev_2.39-0ubuntu8.9_riscv64.deb "$S"
# `libgcc_s.so.1` ships in a package whose version does not track glibc's; the
# board already has the file, and one `scp` is cheaper than finding the deb.
scp user@starfive.lan:/usr/lib/riscv64-linux-gnu/libgcc_s.so.1 \
  "$S/usr/lib/riscv64-linux-gnu/"
ln -sfn libgcc_s.so.1 "$S/usr/lib/riscv64-linux-gnu/libgcc_s.so"
ln -sfn usr/lib "$S/lib"

export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_RUSTFLAGS="\
-C link-arg=--sysroot=$S \
-C link-arg=-B$S/usr/lib/riscv64-linux-gnu \
-C link-arg=-L$S/usr/lib/riscv64-linux-gnu"

cargo build -p xtask --release --target riscv64gc-unknown-linux-gnu
```

Three link arguments rather than the Pi's four: there is no gcc directory to add,
because `libgcc_s` went into the multiarch directory beside libc instead.

Two things about the invocation there. `--skip llvmpipe` is the Pi's flag and is
wrong here -- this image's software driver calls itself `softpipe`, the skip
matches any part of the name a device gives itself, so the Pi's command benches
the software rasterizer on this board. And the account must be in `render` or EGL
cannot open `/dev/dri/renderD128`, falls back to softpipe, and reports a GLES
figure that measures Mesa.

Vulkan does not depend on that group -- enumeration succeeded before it was set --
but it is not indifferent to the node either: sampling `/proc/<pid>/fd` while
`vulkaninfo` runs shows the ICD holding `renderD128` once it can. So the group is
worth setting on both counts, and this entry used to say Vulkan "does not care",
which was true only of whether it works.

The account was added to `render` on 2026-09-29, and the consequence was not the
six GLES rows this entry expected. See below: they arrived, and they are skipped
for a different reason.

### What it measured, 2026-09-29 at bf6bbbd

Desktop stopped, all four governors pinned to `performance` -- which matters more
here than anywhere: `ondemand` idles these cores at 750 MHz against 1.5 GHz
pinned, so an unpinned run halves the clock rather than nudging it. Board at 44.5
C before and 46.5 C after, no heatsink, load 0.11. Three runs, then `--check`
three times over.

**It is the steadiest board here.** All six timed rows inside six tenths of a per
cent across three runs, against the Pi 5's three tenths on its best rows and
worse on others, and with none of the bimodality that makes the Pi want three
runs a side. The reason is dull and worth copying: no desktop, and one governor
state. The exception is `recording / distance field`, the row the bench marks
noisy, which moved fifteen per cent between runs because it is a millisecond on a
slow core and a scheduler hiccup is a large fraction of one.

That count is six because the frame was one row at the time. It is nine now, and
one of the three added rows does not hold to six tenths -- see below. The claim
above is about these six and stays true of them; it was never a property of the
board that would survive adding rows to it.

| route | PowerVR BXE-4-32 | V3D 7.1.7.0 | draws |
|---|---|---|---|
| distance field, 1 sample | 26.617 ms | 8.852 ms | 160 |
| tessellated, 4 samples | 26.874 | 4.525 | 1 |
| tessellated, 1 sample | **10.242** | **3.688** | 1 |
| stroked field, 1 sample | 33.072 | 9.431 | 160 |
| stroked path, 1 sample | **5.569** | **1.184** | 1 |
| full frame, mixed content | 63.919 | 13.908 | 12 |

### Two thirds of that frame is one gradient, measured 2026-09-29 at 1ef0205

The 63.9 ms row above is a budget four times a 60 Hz period, and until this run
nothing attributed it. The frame is now measured in four stages, each adding one
element to the one before, so the difference between two rows is what that element
cost. Same conditions as the run above -- desktop stopped, all four governors
pinned, 46.0 C before and 50.5 C after, load 0.19, three runs and `--check` three
times over.

| stage | PowerVR BXE-4-32 | draws | this element cost |
|---|---|---|---|
| gradient ground | 43.149 ms | 1 | **43.149** |
| plus cards | 45.498 | 4 | 2.349 |
| plus shadows | 48.717 | 7 | 3.219 |
| full frame, mixed content | 64.019 | 12 | 15.302 |

**The gradient ground is 67 per cent of the frame in a single draw.** Three
rounded rectangles and three blurred shadows together are 5.6 ms, or under nine
per cent, and the blurred layer over the top is 15.3.

That is a statement about this scene at this size before it is one about the
renderer: the ground is one draw covering every one of 2.07 million pixels, and
the cards cover a fraction of it, so the two are not comparable per element. What
it does say is where to look, and it is not the shapes or the blurs that put
this board over budget. A full-screen tabulated gradient is one draw that reads a
ramp and writes every pixel, which is the most bandwidth the frame asks for in one
place, on the slowest part measured here -- 4.6 times V3D on this same frame.

It also relocates the earlier finding. The draw-count work above moved the
`distance field` row by nothing at all here, and this says why that was never
going to rescue the frame: the frame's cost is not in its draw count either. One
draw is two thirds of it.

Read the deltas only within one device on one run. The `gradient ground` row is
the one device row on this board that does not hold to a third of a per cent --
it spread 1.6 across three runs where every other held under 0.4 -- so it carries
a 2.0 tolerance in the baseline, and the differences taken from it are worth about
one decimal.

**Confirmed on V3D, 2026-09-30 at 50b64f4, and it leans harder there.** The Pi 5's
stage rows were recorded the same way, after a reboot with the governor pinned. The
figures below are the baseline's current ones, first re-recorded 2026-10-01 at e026ac8 and
re-recorded three times since, most recently at 1502b2a, and they have moved by a tenth
of a per cent or less across all four -- none of the rectangle route flip, the convexity walk
or the occlusion culling touches this scene, whose cards are rounded and analytic:

| route | ground | plus cards | plus shadows | frame | ground's share |
|---|---|---|---|---|---|
| Pi 5 Vulkan | 10.423 ms | 11.066 | 11.912 | 13.928 | **74.8%** |
| Pi 5 GLES | 11.768 | 12.446 | 13.329 | 14.859 | **79.2%** |
| VisionFive 2 Vulkan | 43.149 | 45.498 | 48.717 | 64.019 | **67.4%** |

So a single full-screen five-stop gradient is three quarters of the frame on V3D and
two thirds on PowerVR -- two unrelated tile architectures, the same answer, and the
one with a mature driver is the one that leans on it more. It is not a PowerVR quirk.
The three cards and their three shadows together are between nine and eleven per cent
of the frame on every row here -- 10.7 and 10.5 on the Pi, 8.7 on the VisionFive 2 --
so the shapes are not where the frame goes on any of them.

**What that share is made of is the next section, and it is not what this paragraph
originally said.** The obvious reading -- that a gradient sampling a ramp texture per
fragment is where three quarters of the frame goes, so a different gradient path
would return it -- is wrong twice over. Four fifths of that draw is the fill
underneath, and of the evaluation that remains, the ramp texture is the cheaper of
the two methods available rather than the dearer. Read on before reaching for
`non-parity.md` 1.

### Four fifths of that gradient is fill, and the ramp texture is the cheap half

Measured on the Pi 5, 2026-09-30, after the rows above: same conditions, 57.6 C at
the start, `throttled=0x0` and 2400000 throughout, three runs agreeing to within
0.08 per cent. **This is the measurement that says what the 75 per cent above is
made of, and it contradicts what this file and `non-parity.md` previously implied
about it.**

The ground was drawn three ways, full screen at 1920x1080, in one run so the rungs
are comparable: a flat fill, which evaluates nothing; the same gradient with four
stops, which is `MAX_STOPS`, so the colors ride in the paint block and the shader
walks them; and with five, one past it, so the recorder bakes a 256-texel ramp and
the shader takes a filtered fetch per fragment. Five stops is what the bench's
ground uses, and the probe reproduced `frame, gradient ground` to three
thousandths of a millisecond, which is what says the rungs are the same draw.

| route | flat fill | 5 stops (ramp texture) | 4 stops (paint block) |
|---|---|---|---|
| Pi 5 Vulkan | 8.494 ms | 10.424 | **14.958** |
| Pi 5 GLES | 8.843 | 11.767 | **16.308** |

Two results, and the second was not the expected one.

**Most of the gradient is not the gradient.** Fill alone is 81 per cent of the
five-stop figure under Vulkan and 75 under GLES. Evaluating the ramp adds 1.930 ms
on Vulkan and 2.924 on GLES -- so of a 13.911 ms frame, everything to do with
deciding a gradient's color is 13.9 per cent, and the rest of that draw is the cost
of covering two million pixels once.

**The ramp texture is the cheaper way to evaluate it, by a factor of three.** The
four-stop path walks its stops per fragment and costs 6.464 ms of evaluation
against the ramp's 1.930 -- 3.3 times as much on Vulkan, 2.6 on GLES. A filtered
fetch from a 256-texel table that fits in any texture cache beats a per-fragment
walk with comparisons and interpolation on this hardware, and it beats it by more
than the whole remaining evaluation cost.

So `non-parity.md` 1 is not the debt it reads as, and this file said the opposite of
the truth about it for two entries. Upstream carries 256 stops in uniforms and
reaches a texture only past that; this renderer reaches a texture past four. **The
measurement says the renderer's side of that is faster here**, and closing the gap
-- raising `MAX_STOPS` toward upstream's 256 -- would move every gradient between
five and two hundred and fifty-six stops onto a path 3.3 times more expensive in
its evaluation. That is exactly the kind of question the opening of
`non-parity.md` says is answered against the target devices rather than against
upstream's shape.

It also bounds `non-parity.md` 19, which has since landed. A vertex-interpolated path
does no per-fragment gradient work at all, so the most it can recover is the evaluation
share: about 1.9 ms of a 13.9 ms frame under Vulkan, 2.9 of 14.9 under GLES.

**Measured 2026-10-05, and the bound above does not hold.** One binary, the route
turned off by an environment variable so both sides are the same code and the same
layout, three runs a side on a Pi 5 with all four cores pinned to `performance`,
nothing throttled, load zero, `--skip llvmpipe`, from `/tmp`:

| row | Vulkan, route off → on | GLES, route off → on |
|---|---|---|
| `stacked, wash` | 9.804 → **4.519** ms, −54% | 11.107 → **4.866**, −56% |
| `stacked, plus panels` | 4.057 → **3.667**, −9.6% | 5.110 → **4.634**, −9.3% |
| `stacked interface` | 4.770 → **4.377**, −8.2% | 5.995 → **5.525**, −7.8% |
| `full frame, mixed content` | 13.194 → 13.196, -- | 14.065 → 14.063, -- |

Medians of three; the spread within a side is at most 0.013 ms against gaps of 0.4 to
6.2, so the clusters do not come close to overlapping and the bimodality this board
shows on other work does not arise. `full frame` is the control: its wash is diagonal,
so it never takes the route, and it does not move -- which is what says the switch
changes only what it claims to.

**Two things to read out of it, and the first contradicts the paragraph above.** The
wash alone saves 5.285 ms under Vulkan where the ceiling said at most 1.930, so the
evaluation-share argument underestimates what this route removes. Why is not settled:
the 1.930 figure was taken on a differently shaped frame, and with the route the
material shades as a solid and takes the cheapest branch in `shade()` rather than the
gradient chain, so more than the fetch goes away. That is a candidate and not a
measurement, and it is left as one.

The second is that **an interface frame saves an eighth of what its wash does** --
0.39 ms against 5.29. Occlusion culling is why: the bar, the sidebar and the panel
cover most of the wash, and culling already removed those fragments, so the gradient
work the route would have saved is work that was no longer being done. The two
optimizations overlap, and the order they landed in is why this looks small.

**These numbers are not gated, which is a weakness and is stated rather than
hidden.** They came from a throwaway probe -- three extra rows built from the
ground alone with the stop count as a parameter -- run on the board and then
reverted, so no baseline holds them and no test will notice if they rot. They are
here because the conclusion changed two entries and the evidence should be findable,
not because this is the way to keep a number. Making them permanent means three
more bench rows and a re-recording on both boards; the probe is twenty lines and
`git log` for this paragraph has it.

### A square rectangle was on the wrong route, by thirty per cent

Measured 2026-09-30, interleaved: two binaries differing only in which route
`draw_rect` takes for an unrounded rectangle, alternating in one session on each
board. Pi 5 pinned at 2400000 with `throttled=0x0`; VisionFive 2 with its desktop
stopped and governors pinned.

`draw_rect`'s own comment gave the reason for the analytic route: a shape computing
its own coverage spares the pass multisampling, "four times the fill and four times
the bandwidth saved on a frame made mostly of rectangles". `stacked interface` is
eleven rectangles, so it can test that.

| frame | route | Pi 5 V3D | VisionFive 2 | x86-64 |
|---|---|---|---|---|
| stacked, plus panels | analytic, 1 sample | 18.767 ms | 74.083 | 2.216 |
| | tessellated, 4 samples | **13.687** | **56.780** | **1.631** |
| stacked interface | analytic, 1 sample | 24.508 | 93.981 | 2.758 |
| | tessellated, 4 samples | **16.077** | **66.100** | **1.786** |

Tessellated wins by 34 per cent on V3D, 30 on PowerVR, 35 on x86-64. Three
architectures, one direction.

`full frame, mixed content` is the control: 13.917 against 13.929 on V3D, 64.066
against 64.157 on PowerVR. Its cards are *rounded* rectangles and take the analytic
route either way, so the switch moved what it was meant to and nothing else.

The arithmetic the comment had wrong. Multisampling shades once per pixel and pays in
attachment bandwidth and a resolve, which is 1.23 times on V3D and 2.62 on PowerVR
rather than four. The analytic route pays a distance-field evaluation per fragment
across a quad covering the shape, and for a rectangle that field evaluates to a
constant.

**Flipped 2026-10-01, for fills only.** What that cost is measured too, on the edge of a
rotated rectangle: the field gives 70 distinct partial coverage levels across 268 edge
pixels and the area to 0.0015 per cent, the tessellated route gives 3 levels -- 64, 128,
191 -- across 164 pixels and 0.028 per cent. Near-continuous against three steps, both
areas right. And three steps is the edge upstream gives a rectangle:
`FillRectGeometry::GetPositionBuffer`, read at tip on 2026-10-01, emits a four-vertex
triangle strip under `Mode::kNormal`, with no fragment-evaluated rectangle fill upstream
at all -- so the flip closed a divergence.

Strokes stay on the field. What was measured is a fill, and the stroke route is careful
about a corner the field would otherwise round -- a nine-wide miter differs by 253 of 255
on the corner pixel.

Confirmed here after the flip, interleaved against the previous commit: `stacked
interface` 2.760 ms before against 1.791 after on x86-64 Vulkan, two runs a side with no
overlap, which reproduces the 2.758 and 1.786 above.

The cost baseline did not move. Both routes emit four vertices and six indices, which is
what `draw_rect`'s old comment meant by the vertex count being the same either way --
the difference was never geometry.

It also delivered `non-parity.md` 21 its occluders: the stacked frame now has eleven safe
occluders out of twelve draws, where it had none.

### Mesa's GLES arrived on this board and is still not wanted in the baseline

The `render` group turned GLES on, and the rows it produces are a trap the
baseline's own columns cannot show.

Both backends report the device as `PowerVR B-Series BXE-4-32`. They are not the
same stack. Vulkan is Imagination's DDK (`libVK_IMG.so`). GLES is Mesa:
`EGL_LOG_LEVEL=debug` shows the loader opening
`/usr/lib/riscv64-linux-gnu/dri/pvr_dri.so`, Mesa's in-progress PowerVR Gallium
driver, because `50_mesa.json` is the only EGL vendor on this image and there is
no IMG GLES. On the Pi 5 a `vulkan` row and a `gles` row are one Mesa either
side; here they would cross a vendor boundary under identical device names.

The numbers make that worse rather than better:

| route | gles (Mesa pvr) | vulkan (IMG DDK) |
|---|---|---|
| tessellated, 1 sample | 7.289 ms | 10.181 |
| stroked path, 1 sample | 3.100 | 5.567 |
| full frame, mixed content | 57.940 | 64.025 |

Mesa's GLES is *faster* on every row. The tempting reading -- that the community
driver beats the vendor's -- is not available: this is also a different renderer
backend, with its own pass setup and its own sample handling, so the gap
attributes to neither the driver nor the architecture. Two variables moved.

So `--skip gles` joins `--skip softpipe` in the documented invocation, and the
baseline stays Vulkan-only. Take a GLES row here only with a question about
Mesa's pvr driver specifically, and label it as that driver rather than as this
board.

### The field costs more than the triangles on both, and that is now a fact about tiling

A filled rounded rectangle through the analytic field costs 2.60 times the
tessellated one here and 2.40 times on V3D. Stroked, it is 5.94 times here and
7.96 on V3D. Two unrelated tile architectures, the same direction and nearly the
same magnitude.

A mechanism was proposed for it here and has since been tried and mostly
disproved, which is worth keeping in full rather than quietly replacing.

The proposal was that the two routes do not submit the same number of draws.
Every tessellated shape carries the same solid material, so the batch merged all
hundred and sixty into one; an analytic shape carried its geometry inside its
material and merged with nothing. The comparison was one draw against a hundred
and sixty, and this entry concluded that reading it as fragment work against
fragment work was reading it wrong, and that moving a shape's parameters out of
its material was worth more than any policy picking between routes.

**The move was made and the second half of that is not what happened.** A rounded
rectangle now carries its own space on its vertices rather than in its material,
so identical shapes merge. Pinned, three runs a side:

| device | draws | distance field | stroked |
|---|---|---|---|
| Adreno 640 | 160 -> 44 | 13.78 -> 13.15 ms | 15.33 -> 14.50 ms |
| PowerVR BXE-4-32 | 160 -> 44 | 26.78 -> 26.78 | 33.00 -> 33.31 |

About five per cent on the Adreno and **nothing at all on the PowerVR**. Taking
away nearly three quarters of the draws moved that part's frame by less than the
run-to-run spread, so on it the comparison *was* fragment work against fragment
work, and this entry had that backwards. The field is simply expensive there --
the same shapes take it twice what they take the Adreno.

Forty-four rather than one because the grid is not a single size: `shapes()`
builds each rectangle as `left + width` less `left`, which in f32 gives three
widths and three heights, and only exactly equal sizes merge. Content with
genuinely identical shapes collapses further, so the draw count above is a floor
on what merging can do rather than a ceiling.

What survives is the narrower claim: the draw count is part of what the analytic
route pays on one part and none of what it pays on another, and a mechanism that
holds on two tile GPUs can still fail to explain a third.

### Multisampling is where they disagree, and it inverts

Four samples against one, on the same tessellated shapes:

| board | 4 samples | 1 sample | cost of MSAA |
|---|---|---|---|
| V3D 7.1.7.0 | 4.525 ms | 3.688 ms | **1.23x** |
| PowerVR BXE-4-32 | 26.874 ms | 10.242 ms | **2.62x** |

On V3D four samples are nearly free, which is the property that makes
multisampling the obvious way to antialias on a tiler: the samples live in tile
memory and resolve on chip. On PowerVR they cost more than twice the frame.

This is the entry to point at when someone -- including whoever wrote the
sentence above -- generalizes from one tile GPU to tile GPUs. "Cheap on a tiler"
was a V3D fact wearing an architecture's clothes. Sample count is therefore the
one decision in this renderer that wants to follow the device rather than sit in
the code, and it is currently `samples: 4` written into `Canvas::new`.

**`SampleCounts` cannot carry that decision, which this entry used to imply it
could.** `framebufferColorSampleCounts`, read on every device on the bench:

| device | Vulkan | color sample counts |
|---|---|---|
| V3D 7.1.7.0 | 1.3.305 | 1, 4 |
| PowerVR BXE-4-32 | 1.3.225 | 1, 2, 4 |
| Adreno 640, SA8155P | 1.1.128 | 1, 2, 4 |

`max()` is four on all three. It returns the same answer for the device where
four samples cost 1.23x and the device where they cost 2.62x, because it reports
what is *supported* and there is no query in Vulkan or GLES for what is *cheap*.
A part advertising eight would be told to take eight, which is the wrong
direction on the evidence above. So wiring it in would not have implemented the
decision; what carries it today is `Canvas::with_samples`, which the executor
already threads from a scene's own count.

What the masks do say is narrower and more useful. **Two samples exist on both
devices where four are dear, and not on the one where four are nearly free** --
the middle setting is available exactly where it might help, and absent where it
would not.

### Two samples do not rescue PowerVR

Measured by adding a two-sample tessellated route to the bench, running it, and
taking the route back out; the rows below are the tessellated rounded rectangle
at each count, on Vulkan. PowerVR with the desktop stopped and all four
governors pinned, 43.5 C before and 49.3 C after, three runs. Adreno with the
eight CPU governors and the `kgsl-3d0` devfreq pinned to `performance`, three
runs, restored to `schedutil` and `msm-adreno-tz` after. V3D's pair is the
committed baseline, and it has no two-sample row because the device does not
offer one.

| device | 1 | 2 | 4 | 2 over 1 | 4 over 1 |
|---|---|---|---|---|---|
| V3D 7.1.7.0 | 3.688 ms | — | 4.525 ms | — | **1.23x** |
| Adreno 640 | 6.92 | 7.49 | 8.24 | **1.08x** | **1.19x** |
| PowerVR BXE-4-32 | 10.25 | 18.12 | 26.93 | **1.77x** | **2.63x** |

**PowerVR pays per sample.** One to two costs 7.87 ms and two to four costs
8.81 ms -- two nearly equal steps, which is what a part resolving somewhere other
than tile memory looks like. Adreno and V3D both spend under a quarter to go from
one sample to four, and the Adreno's two-sample row buys almost nothing because
there is almost nothing to buy.

So the middle setting is not the answer it looked like. Dropping four to two on
PowerVR recovers 8.81 ms of the 16.68 ms that antialiasing costs there, a little
over half, and leaves the row still 1.77x its unantialiased self. **Sample count
is not the lever on that part; the antialiasing strategy is.** Which also means
the device-property framing this section began with resolves to something smaller
than it promised: two of the three parts do not care what the count is, and the
third is not fixed by changing it.

Worth stating for the vertical this renderer aims at: the automotive part is one
of the two that do not care. Four samples cost it nineteen per cent.

The Adreno's `1.1.128` is worth noticing while the table is here: the automotive
part is the one sitting on the Vulkan 1.1 floor this renderer targets, where the
other two are well past it. The floor is load-bearing rather than theoretical.

### The number that outranks the routes

A mixed frame of 1080p content costs **63.9 ms** here: sixteen frames a second,
nearly four times a sixty-hertz budget, where the Pi 5 does the same frame in
13.9. Tuning the fill route on the hundred-and-sixty-rectangle scene moves
sixteen milliseconds in a synthetic frame; this is the realistic one, it is
reported as a single number over twelve draws, and nothing attributes it.

That is the case for per-route rows in the bench rather than for per-device
policy in the renderer. A blur, a shadow, a tabulated gradient and a layer are
all in that frame and none of them has a row of its own, so on the board where
the frame is furthest over budget there is no way to say what it is spending on.

Two smaller findings from the same runs. These cores are about ten times slower
per core than the Pi 5's A76 -- the field's recording row is 0.87 ms here against
0.099 -- and the stroker is the most expensive recording row in the file at 6.33
ms, eight times the tessellated fill's, which on a slow core is a cost in its own
right rather than a detail of the comparison. And this device reports no
advanced-blend support, so all fifteen of those modes refuse here.

### A cap set from the desktop was wrong by four points, measured 2026-10-01 at 0cebdf4

`emblema-geometry`'s ear-clipping fast path is bounded by what it costs to *prove* a
contour simple, and that bound is a property of the machine rather than of the
algorithm. The cap went in at twenty-four, from a sweep taken here on x86-64 where the
two routes cross at thirty. The Pi 5 says they cross at twenty.

`cargo run --release -p emblema-geometry --example ear-crossover`, cross-built with the
recipe at the top of this file and run from `/tmp`, governor `performance`, 57 to 59 C,
`throttled=0x0`, 2.4 GHz throughout, nothing else on the board. Three runs, which agreed
to within two hundredths:

| points | x86-64 | Pi 5, A76 |
|---|---|---|
| 8 | 1.59x | 1.57x |
| 12 | 1.53x | 1.25x |
| 16 | 1.39x | 1.19x |
| 20 | 1.25x | 1.00x |
| 24 | 1.15x | **0.93x** |
| 32 | 0.94x | 0.76x |

So the shipped cap of twenty-four was serving twenty- and twenty-four-point contours on
the reference board at a loss, and the cap is now sixteen -- under the nearer crossover
rather than under the one machine that was measured first.

The A76 is the narrower machine here, which is the part worth carrying forward: it wins
less at every size and runs out sooner. Seeing the crossover at all needs
`MAX_EAR_POINTS` raised first, since a contour past the cap takes lyon in both columns
and the ratio reads 1.00 -- which is what the rows above twenty-four read on the shipped
build, and is how that build was confirmed to be declining them.

**The general lesson, since this is the second cap this route has had wrong.** A margin
guessed from one machine is not a margin. The first cap was five hundred and twelve,
which let `earcutr` hang; the second was twenty-four, which the board measured at a
loss. Both were set without the board.

### Multisampled depth is what V3D cannot afford, measured 2026-10-02 at 5b85aaa

`non-parity.md` 21 wanted a depth buffer to reorder opaque draws, and said the
attachment was "already allocated and already paid for on every target that clips". The
frame the prize was measured on clips nothing -- `stacked interface` records twelve draws,
none stencilled and none scissored -- so it would pay for the attachment from scratch.
Measured before building anything, with two binaries differing only in whether the pass
carries a depth-stencil attachment and no reordering in either:

| row | samples | without | with | |
|---|---|---|---|---|
| distance field | 1 | 8.212 ms | 8.217 | +0.1% |
| tessellated | 1 | 3.696 | 3.702 | +0.2% |
| tessellated | 4 | 4.530 | **18.51** | **+309%** |
| stacked, wash | 4 | 10.42 | 24.59 | +136% |
| stacked interface | 4 | 16.07 | 30.20 | +88% |

Three runs a side, interleaved, 58.2 to 63.7 C, `throttled=0x0`, 2400000 throughout,
load average 0.00. The clean numbers reproduce the baseline to a hundredth.

**At one sample the attachment is free and at four it costs four times the pass.** That
is the whole finding, and it is not what the entry assumed. x86-64 puts the same cost at
9.4 per cent on `stacked interface` -- 1.790 against 1.958, three runs a side -- so this
is a property of the tiler rather than of depth testing, and a desktop measurement would
have waved it through.

Not established here: *why*. A 1920x1080 four-sample D24S8 alongside four-sample color is
more than this tile memory holds, and either smaller tiles or a spill would explain the
factor. Which one it is wants a counter this project does not read, and the number above
does not depend on knowing.

**It leaves entry 21 in a bind that this repository created.** A draw is a safe occluder
when the rasterizer decides its coverage, and asking the rasterizer for coverage is what
raises the pass to four samples. Before the rectangle route flip the stacked frame was one
sample and had no occluders; after it the frame has eleven and is four samples. The
property that makes reordering possible is the property that makes its attachment
unaffordable, on this device.

So reordering by depth is not the way here, and the entry records the alternative it
already named: overlap analysis on the CPU, which needs no attachment and is indifferent
to the sample count.

### Culling hidden pixels by scissor, measured 2026-10-02 at 452b783

The route `non-parity.md` 21 was left with after the depth attachment was ruled out above.
`Batch::cull_occluded` confines each draw to the pixels no later opaque draw replaces --
order preserved, no attachment, indifferent to the sample count.

Three runs a side, interleaved, 57.1 to 63.1 C, `throttled=0x0`, 2400000 throughout, load
average 0.08, nothing else on the board:

| row | device | before | after | |
|---|---|---|---|---|
| stacked, plus panels | Vulkan | 13.686 ms | **4.478** | −67% |
| stacked interface | Vulkan | 16.074 | **5.198** | **−68%** |
| stacked, plus panels | GLES | 14.977 | **5.195** | −65% |
| stacked interface | GLES | 17.304 | **6.134** | **−65%** |
| full frame, mixed content | Vulkan | 13.924 | 13.923 | -- |
| stacked, wash | Vulkan | 10.423 | 10.423 | -- |

Sixty-two frames a second to a hundred and ninety-three. `full frame` is the control --
its cards are rounded, so nothing in it occludes -- and `stacked, wash` is a single draw
with nothing to cull against. Both hold still to a hundredth, which is what says the change
moved only what it was meant to.

**It beats what the depth route was estimated to save.** `non-parity.md` 21 put reordering
at 9.81 ms, from culling the wash alone. This takes 10.88, because the panel under the rows
is hidden too and a scissor does not care how many layers deep the covering goes.

**The draw count was the worry and the board says it is not one.** Twelve draws become a
hundred and thirty-one, and a tiler charges binning per draw that an immediate-mode renderer
does not -- so the expectation going in was that `MAX_PIECES` would want lowering here. The
opposite: at a cap of sixteen the frame keeps 68 draws and reads **6.975 ms**, against 5.198
at thirty-two. Fewer draws is slower, by a third of the remaining frame. The cap stays at
thirty-two, now set from this board rather than from a desktop -- which is the lesson the
ear-clipping cap taught twice.

What it costs is processor time, and the ratio is not close: the recording row goes from
0.016 ms to 0.036, so twenty microseconds buy nearly eleven milliseconds of fill.

x86-64 agrees on direction and understates the size, which is the usual way round for a fill
change: `stacked interface` 1.790 to 0.838 on Vulkan and 2.100 to 1.078 on GLES, about half
rather than two thirds.

The pictures are identical, which is checked rather than argued:
`culling_hidden_pixels_changes_no_pixel` renders six scenes with the pass and without it and
compares every byte. One of them puts an opaque bar half a pixel off the grid, because
rounding an occluder outward instead of inward passes every other scene and leaves one row
of seam.

### Culling on two more architectures, measured 2026-10-02 at 7155090

The Pi 5 numbers above are V3D. Two other boards, same two binaries, to see whether the
result is a property of that tiler or of the idea.

**SA8155P, Adreno 640, Vulkan 1.1.128.** Three runs a side over `adb`:

| row | before | after | |
|---|---|---|---|
| stacked, plus panels | 22.43 ms | **9.62** | −57% |
| stacked interface | 27.23 | **11.03** | −60% |
| full frame, mixed content | 24.52 | 24.57 | -- |
| stacked, wash | 15.21 | 15.17 | -- |

Three tile architectures now agree on direction and roughly on size: 68 per cent on V3D,
60 on Adreno. The controls hold on both. This board is not quiet -- it runs its own
services and the ninety-ninth percentiles are wide -- but the medians repeat to under a
per cent across runs, which is enough for a two-thirds effect.

**i.MX8MP, Vivante GC7000UL: the device half does not run on a stock driver stack, and that
is not this change's doing.** It *does* run behind an inlining layer; the section below has
that, and it is the correction to the sentence this used to open with, which was "does not
run at all". `cargo xtask bench` segfaults in the Vulkan section -- exit 139, `sig=11`
in the kernel audit log -- and the binary built from the commit *before* occlusion culling
segfaults in the same place. So it is a standing fault on that device rather than something
to attribute here.

`xtask report` succeeds, so the device opens and its capabilities read back: Vulkan 1.3.0,
max texture 8192, sample counts 1 and 4. Vulkan is the only path this renderer can take there,
so there is no second one to compare against -- but **not because the board has no GLES**, which
is what this said first. It has `libGLESv2` and that advertises
`GL_KHR_blend_equation_advanced`; what it has not got is `EGL_MESA_platform_surfaceless`, the
one way `DisplayTarget` knows how to ask. The GLES section below has the reading.

### The in-source workaround does not work here, measured 2026-10-07

One candidate rule for this crash is narrow enough to dodge in the shader
source: that it needs an `OpCompositeConstruct` taking an
`OpFunctionParameter` result as an operand, in a non-entry-point function. If
that were the whole rule, making the operand any other instruction's result
would avoid it, and no driver-side workaround would be needed at all. **It was
tried. The crash does not move.**

Our SPIR-V did carry the pattern, twice, both in `solid.wgsl`:
`gradient_color` building `vec2<f32>(t, 0.5)` from its `t` parameter, and
`rounded_rect_distance` splatting its `radius`. Routing both through a `var`
makes the operand an `OpLoad` and takes the module to **zero** instances, which
a scan of the built SPIR-V confirms on the aarch64 artifact actually shipped.

The board then crashes exactly as before: `exit=139`, `sig=11` in the audit
log, on the first test that creates a pipeline, and `gdb` gives the same frame
it always gave --
`VIR_Shader_CompositeConstruct` in `libVSC.so`, under `gcSPV_Decode`, under
`vkCreateGraphicsPipelines`. Not a different crash; the same one.

**So the trigger is broader than that pattern, or there is a second one.**
`SOLID_SPV` holds **51 `OpCompositeConstruct` in 18 non-entry functions**
against 2 in its two entry points, so a rule about *called functions* rather
than about *parameters* would fit everything measured just as well.

### The whole Vulkan backend runs there behind an inlining layer, 2026-10-07

An implicit Vulkan layer that runs SPIRV-Tools' exhaustive inlining pass over
every shader module before the driver sees it is available for this board. With
it loaded, **the Vulkan backend works**:

| binary | with the layer | with `VIV_SPV_INLINE_DISABLE=1` |
|---|---|---|
| `draw` | 10 passed | `exit=139` |
| `batch` | 8 passed | -- |
| `pixels` | 8 passed | -- |
| `blend` | 10 passed | -- |

Thirty-six tests on a device this document called unable to create a pipeline.
Six modules get rewritten per run. The layer is a prebuilt binary that this
repository does not ship or track, so nothing here depends on it and the A/B
above is how a claim about it gets made: loaded (the loader's own
`Insert instance layer` line), doing work (an `inlined module` line per
module), and still crashing when disabled.

### The second trigger, named: an extract from a value parameter

Found by bisection rather than by another guess, since guessing had produced
two wrong shapes already. Stubbing each of `solid.wgsl`'s thirty-nine callable
functions to a constant return and halving put it in `shade` alone; cutting
`shade`'s body at function-body depth put it between its lines 126 and 154 --
the `switch` on the material kind. The line is

```wgsl
case 7: { return rounded_rect_coverage(select(in.clip, vec3<f32>(in.uv, 1.0), ...)); }
```

`in` is `shade`'s value parameter, `in.uv` an extract from it, and
`vec3<f32>(in.uv, 1.0)` a composite construct taking that extract. A probe
variant of exactly that shape crashes; so **the rule is an
`OpCompositeConstruct`, in a non-entry-point function, one of whose operands is
a value parameter *or a component extracted from one*.**

| shape | Vivante |
|---|---|
| construct in an entry point | 0 |
| construct in a called function, operand a loaded global | 0 |
| operand a load *through a pointer* parameter | 0 |
| operand another call's result | 0 |
| **operand extracted from a value parameter** | **139** |
| **operand the value parameter itself** | **139** |

The extract is what makes it reach a renderer, and it is why every earlier
attempt missed: `solid.wgsl` holds **seven** of these, in `gradient_color`,
`gradient_space`, `rounded_rect_distance`, `blend_tint` (two), `dithered` and
`shade` -- and the two in the direct form were the only ones a scan for
"operand is a parameter" could see. Removing those two changed nothing because
five remained. The other four modules have none, which is why only `solid.wgsl`
crashes.

Bisection cost eight board runs and the board answers in seconds. The two
guesses before it cost more than that and were both wrong, which is the whole
argument for having gone to it earlier.

### The in-source fix, done: the pipeline compiler stops crashing

Each of the seven operands now goes through a `var`, which makes it a load --
`called-extract-var` in the probe is that shape and passes. Six of the edits
are a local `var`; the seventh needed a function, `rounded_rect_space`, because
`shade`'s `case 7` arm is an expression with nowhere to put one.

**Measured on the i.MX8MP with `VIV_SPV_INLINE_DISABLE=1`, so no layer is in
play:**

| binary | before | after |
|---|---|---|
| `draw` | `exit=139` | 9 passed |
| `batch` | `exit=139` | 8 passed |
| `pixels` | `exit=139` | 8 passed |
| `blend` | `exit=139` | 10 passed |

Thirty-five tests, on a stock driver stack, where the first pipeline used to
segfault. **That is a claim about pipeline creation and not about the board**,
and the section below is why the distinction matters: `cargo xtask bench`
reaches a *second* defect that this does not touch.

**It changes no pixels.** The gate's seventy-four corpus scenes still match
their stored images, and every cross-backend and cross-device comparison is
unchanged -- which is the thing to check, since seven edits to a shader for a
driver's sake is exactly where a quiet rendering change would hide.

Checked on a fourth driver family rather than assumed: on a Raspberry Pi 5,
`cross_backend` passes eight of eight and `catalog` thirty-three of
thirty-three, which compares the plates across V3D's Vulkan and its GLES. So
the four the bench can reach all agree -- RADV and radeonsi through the gate,
lavapipe and llvmpipe beside them, Vivante through its own suites, and V3D
here.

**What it costs**: `solid.wgsl`'s SPIR-V goes from 9,974 words to 10,234, two
and a half per cent, and the GLSL gains the same stores and loads. Whether
that moves a frame is unmeasured. `shader-cost-is-a-step-function` says shader
changes can move cost in steps and that the two backends want opposite shapes,
so `cargo xtask bench --check` on a board is what would say, and it has not
been run.

`the_shader_builds_no_vector_from_a_parameter` in `emblema-shaders` is what
keeps the seven from being tidied away by someone who does not know why they
are there. It reads the built SPIR-V rather than the WGSL, covers every module
through a table `build.rs` emits, and names the file and the count when it
fails.

**What that says about the second trigger.** Inlining removes every call, and
it fixes this. So whatever reaches emblema is call-related, like the
documented pattern and unlike it: taking `solid.wgsl` to zero direct
value-parameter operands changed nothing, and removing the calls entirely
fixes everything. The shape is narrower than "any call" and wider than "a
value parameter in a composite construct", and bisecting `solid.wgsl` is still
what would name it.


### The first bench rows from that board, and a second defect under them

`cargo xtask bench` had never produced a row there. It does now, which is the
pipeline fix working, and then it segfaults -- so "Vulkan runs on a stock
stack" is true of the four test binaries and not of the renderer at large.

Eight of fourteen Vulkan rows, 1920x1080, governor pinned to `performance`,
clock already at its 1.6 GHz maximum:

```
vulkan:0 VeriSilicon
  distance field, 1 sample 355.6 ms      stroked path, 1 sample    13.2 ms
  tessellated, 4 samples    74.9 ms      frame, gradient ground   324.0 ms
  tessellated, 1 sample     61.3 ms      frame, plus cards        352.9 ms
  stroked field, 1 sample  389.7 ms      frame, plus shadows      379.2 ms
```

A GC7000UL at three frames a second on content a Pi 5 runs in under a
millisecond. The rows repeat to a tenth across runs, so they are measurements
rather than noise, and they are the first numbers anyone has from this part.

**Then it dies, and not of the decoder.** The crash is in
`libvulkan_VSI.so.1` under `emblema_hal_vulkan::render::record_draw` -- command
recording, not pipeline creation, and nothing from `libVSC.so` or
`libSPIRV_viv.so` in the backtrace. **The inlining layer does not change it**:
same exit, same row. So it is a second defect, unrelated to the composite
construct, and the shader fix neither caused nor cures it. What it blocks is a
full bench row set from this board.

**One more thing that fell out of the A/B.** With the layer loaded the frame
rows are far faster -- `frame, plus cards` 231.9 ms against 352.9, `plus
shadows` 248.2 against 379.2, about a third off -- on the same emblema build,
the only difference being that every shader function has been inlined before
the driver saw it. The no-layer rows repeat to a tenth across three runs, so
the gap is not noise. This driver's own compiler does badly with calls, which
is a different complaint from crashing on them and is worth knowing before
anyone reads these numbers as what the part can do.

### Which it is: the rule is narrow, and it is not ours

`crates/emblema-hal-vulkan/examples/probe-the-composite-rule.rs` settles that.
It builds five modules that differ from each other in one thing, translates
them with naga, reads back out of the SPIR-V what each actually contains
rather than trusting the WGSL, and creates a compute pipeline from one per
invocation -- one per process, since a crash takes the process with it.

| variant | what it has | Vivante | RADV |
|---|---|---|---|
| `entry-only` | the construct, in the entry point | 0 | 0 |
| `called-no-construct` | a called function taking a value, no construct | 0 | 0 |
| `called-no-params` | a construct in a called function, operand a loaded global | 0 | 0 |
| `called-loaded-param` | a construct in a called function, operand a load *through* a pointer parameter | 0 | 0 |
| `called-param-operand` | a construct in a called function, operand the value parameter itself | **139** | 0 |

**The narrow rule is right and the broad one is wrong.** A composite construct
in a called function is fine. A called function taking a parameter is fine.
Loading *through* a pointer parameter and constructing from that is fine. Only
the value parameter used directly as an operand crashes, which is what the
characterization said and what this doubted.

**Which means emblema's crash is a second trigger, not this one.** The fix
above took `solid.wgsl` to zero instances of exactly this shape, verified on
the shipped artifact, and the board still died in the same frame. So there are
two, and the one that reaches this renderer is still unidentified.

Two caveats on the table. The probe builds *compute* pipelines and emblema
dies building a *graphics* one; that does not weaken the conclusion, since what
rules the documented pattern out for emblema is that removing every instance
changed nothing. And five variants is five, not a search -- the second trigger
could be a composite type of parameter, a nesting depth, or something the
graphics path alone reaches.

**What would find it** is bisecting `solid.wgsl` rather than guessing again:
it is the only module that crashes, and halving it is a few runs on a board
that answers in seconds.

No workaround is in the tree. A `var` whose comment claims to dodge a crash it
does not dodge is worse than no `var`, so the change was reverted after being
measured.

**Where it dies, from `gdb` on the board rather than from reasoning:**

```
#0  libVSC.so
#1  VIR_Shader_CompositeConstruct        libVSC.so
#2  libSPIRV_viv.so
#4  gcSPV_Decode                         libSPIRV_viv.so
#5  libvulkan_VSI.so.1
#8  emblema_hal_vulkan ... submit_batch_textured
```

So it is the vendor's SPIR-V decoder, on the first batch that needs a pipeline -- not an
allocation, not multisampling, not contention. The board was idle with nothing holding
`card0`, `card1`, `renderD128` or `/dev/galcore`, and no compositor running, so none of the
usual suspects on that device apply.

**It is the driver, and that took the bare-API reproduction to say.** The backtrace could
not settle it: a crash inside a compiler fits illegal SPIR-V and a compiler bug equally.
`compile-shaders`, an example in `emblema-hal-vulkan`, is that reproduction -- an instance, a
device, `vkCreateShaderModule`, one `vkCreateGraphicsPipelines`, and nothing of this renderer
but the SPIR-V.

What it found, with the renderer's own pipeline layout and the smallest legal everything else:

| module | i.MX8MP | RADV |
|---|---|---|
| `effect`, `effect-image`, `effect-mesh-uv`, `effect-two-images` | compiled | compiled |
| `solid` | **segfault** | compiled |
| `solid` vertex + `effect` fragment | compiled | compiled |
| `effect` vertex + **`solid` fragment** | **segfault** | compiled |

So pipeline creation works on that device, four of the five modules compile there, and what
crashes it is `solid.wgsl`'s fragment entry point. SPIRV-Tools validates all five clean
against both Vulkan 1.0 and 1.1 rules. A driver that crashes on a validated module which
four of its siblings survive and another driver compiles is the driver's fault, whatever is
in the module.

**The first version of that reproduction was wrong, in the direction that matters.** It used
an empty pipeline layout, on the reasoning that less state asks a cleaner question, and it
segfaulted on RADV -- a driver that compiles these shaders every day. A pipeline whose layout
does not cover the resources its shaders declare is invalid usage, so that crash was the
program's own and the conclusion would have been a false accusation. The layout is now the
renderer's: the texture set from `sampling::create_descriptor_layout` so it cannot drift, and
the material set mirrored from `materials::create_layout`, which is `pub(crate)`. **A
reproduction has to be valid before it is evidence**, and an empty layout is the easy way to
forget that.

**What it is not, which is most of the search space.** Probed with descriptor-free fragment
shaders built by `glslangValidator`, each validated by `spirv-val`, run with an empty pipeline
layout -- legal there, because they declare nothing:

| probe | words | i.MX8MP |
|---|---|---|
| chained arithmetic | 214, 739, 4,915, 19,239, **76,531** | all compiled |
| `dFdx`/`dFdy`/`fwidth` | 205 | compiled |
| 32-iteration loop with a branch inside | 319 | compiled |
| twelve-case `switch` | 612 | compiled |
| derivatives inside branchy control flow, computing coverage from a distance | 503 | compiled |

So **not size** -- a 76,531-word module compiles where our 9,347-word one does not -- and not
derivatives, not loops, not switches. Not descriptors or texture sampling either:
`effect-image` samples through a separate image and sampler and compiles with the same
layout.

What is left is the combination in one entry point: many material branches, descriptors and
derivatives together. Narrowing past that means bisecting fifteen hundred lines of WGSL, and
the vendor can do that faster with the above than this project can.

**It is also not something precompiling could avoid, which is worth stating because it is the
first thing suggested.** `impellerc` upstream is "host side tooling that consumes GLSL and
generates libraries", with metadata "to construct rendering and compute pipelines *at
runtime*" -- so it produces SPIR-V ahead of time, which is exactly what `build.rs` already
does here through naga. `SOLID_SPV` *is* the precompiled artifact. The compiler that crashes
is the vendor's SPIR-V to machine code stage inside `vkCreateGraphicsPipelines`, and nothing
portable skips it: a `VkPipelineCache` blob has to be produced by that driver on that device,
so surviving the compile once is a precondition for having one rather than a way around it.

Where upstream does differ is granularity -- many small shaders against this tree's one
`solid.wgsl` carrying every material -- and the size sweep above is the reason not to expect
that to fix *this*. It would change which module is handed over, and might miss whatever the
fault is, but it would not be addressing it.

One observation from the same work, recorded because it means the suite and the bench do not
compile the same shader: `SOLID_SPV` is 10,036 words in a debug build and 9,347 in a release
one, from one source through one naga. The crash is on the release module, which is what the
bench and every board binary carry, and the gate compiles the other.

**Its recording rows do run, and they price the processor side on a slow core.** A
quad-A53 against the Pi's A76:

| row | before | after | |
|---|---|---|---|
| stacked, plus panels | 0.069 ms | 0.082 | +19% |
| stacked interface | 0.097 | 0.195 | +101% |

Ninety-eight microseconds rather than the Pi's twenty, for the same hundred and
thirty-one draws. On V3D that buys eleven milliseconds of fill, so the trade is not close;
on this board the device half cannot say yet, and the honest position is that culling's cost
there is measured and its benefit is not.

### A real controller composites what a real GPU exported, measured 2026-10-06

The comparison below was written against vkms with a software renderer, and the
section after it records why. On a Raspberry Pi 5 it runs against the thing
itself: `vc4` driving a connected HDMI output at 1280x1440, V3D 7.1.7.0
allocating and exporting the frame, source `ARGB8888` at the linear modifier,
destination `XRGB8888`. **All three tests pass** -- the capture is not one flat
color, a quadrant reads back as the bytes it was drawn, and the composition
matches an ordinary offscreen render of the same batch within
`Tolerance::ROUNDING`. `EMBLEMA_WRITEBACK_DEVICE=auto` is what asks for the
machine's own device instead of the software default.

That is the first time anything here has compared a display controller's output
against the renderer's on hardware, and it is also evidence about the
disagreement below: the same test, same code, with a real GPU's export and a
controller that composites by DMA, agrees to within a byte.

**vc4 has two writeback connectors**, `card0-Writeback-1` and `-2`. A note in a
neighboring project said vc4 and vkms had none, which was `modetest` not
setting `DRM_CLIENT_CAP_WRITEBACK_CONNECTORS` rather than the hardware: `ls
/sys/class/drm/` lists them whatever a client asked for.

**The resize storm ran on real modes.** All seven of `kms.rs`'s tests pass,
including the storm cycling 1280x1440, 640x480 and 720x480 four times over,
which is real modesets on a live HDMI output rather than vkms accepting
whatever it is given. So the mode blob swap, the plane rectangles following it
and the ring rebuild are right against a display controller and not only
against the stand-in.

Ten tests take DRM master between the two files and all ten pass. The board was
idle at 60.4 C on the `ondemand` governor, which is fine here because none of
this is a timing measurement.

**`vc4` registers no CRC source.** `/sys/kernel/debug/dri/0` has `crtc-0`
through `crtc-3` and nothing named `*crc*` anywhere beneath it; the same holds
for `drm-rp1-dsi` and `v3d`. So L4's CRC gap wants different hardware rather
than a privilege -- debugfs there is `root:sudo` and readable without a
password, which is how this was checked.

## The controller's own answer disagrees with the render, on one device

`writeback.rs` asks a CRTC to hand back what it composited and compares it
against an ordinary offscreen render of the same batch. It found a disagreement
on the first run, and the disagreement is not in the picture the renderer drew.

Measured on this workstation, 2026-10-06 at d7b9f31, against `card0`, which is
vkms. Mode 1024x768; source an exported `ARGB8888` image at the linear modifier,
stride 4096 and offset 0; destination a dumb buffer at `XRGB8888`, pitch 4096.

With the Vulkan device the machine picks by itself -- RADV on the Raphael
integrated GPU -- **512 of the 768 rows differ, 524,288 of 786,432 pixels, max
delta 255**. The disagreement is not a shift: rows 512 to 767 of the capture
equal rows 512 to 767 of the render exactly, including the diagonal band
crossing them. The rows above hold regularly blocked content that is not the
frame at all -- 159 distinct colors in a sampled grid, mostly dark -- which is
what some other buffer read through the wrong layout looks like.

Four things were ruled out on the way to the mechanism below.

- **It is not the render.** Reading the same exported image back through Vulkan
  gives the color the scene draws in all fifteen pixels sampled across it. The
  GPU wrote the frame; the kernel did not read the frame.
- **It is not the instrument.** With the software Vulkan device exporting
  instead, the capture is bit-identical to the render: 0 of 768 rows differ, and
  the capture holds exactly the five colors the scene draws. Same commit, same
  card, same mode, same code path.
- **It is not a missing flush.** Putting a transfer out of the image -- which
  takes it through a host-read barrier -- between the render and the commit
  changes nothing; still 512 of 768.
- **It is not a fixed size or a fixed fraction.** At 2560x1600 the part that
  agrees is the last 12 rows of 1600, 120 KiB of 16,000. At 1024x768 it is the
  last 256 of 768, 1 MiB of 3,072. Neither two thirds nor two megabytes.

### What it is: the heap the export lands in, measured 2026-10-06

`crates/emblema-present-drm/examples/read-the-export.rs` is the bare-API
reproduction, in the sense `compile-shaders` is one: plain `ash` and `drm`, no
renderer, no shaders, no pipeline. It allocates a linear `B8G8R8A8` image with
an explicit memory type, fills it **from a staging buffer** so the bytes are the
program's rather than a GPU's, and reads it back three ways -- through Vulkan,
through a userspace `mmap` of the exported descriptor, and through `vkms`
compositing it to a writeback connector, which is the kernel reading the same
memory with `dma_buf_vmap`. The pattern's green channel is the row index, so a
row read from the wrong place says where it came from.

```
cargo run -p emblema-present-drm --example read-the-export --release
```

One run, the three placements an exportable linear image accepts here:

```
placement device-local:   type 0, heap 1
  through Vulkan:   matches the pattern
  through mmap:     failed, Operation not permitted
  through the CRTC: 512 rows wrong, first at 0, whose green says row 255
placement both:           type 3, heap 1
  through Vulkan:   matches the pattern
  through mmap:     matches the pattern
  through the CRTC: 512 rows wrong, first at 0, whose green says row 112
placement host-visible:   type 2, heap 0
  through Vulkan:   matches the pattern
  through mmap:     matches the pattern
  through the CRTC: matches the pattern
```

**The middle row is what rules out the obvious answer.** At type 3 a plain
userspace map of the very same file descriptor returns the frame exactly, byte
for byte against what the GPU wrote, and the kernel's own read of it is still
wrong. So CPU-visibility of the pages is not the discriminator.

What is invariant across every series run so far, both usages and both fill
paths: **an export in the host heap is read correctly, every time.** What is
*not* invariant is the rest of it, and an earlier version of this section said
otherwise.

- The amount wrong changes between series: 512 rows of 768, or 256, at the same
  size and placement.
- A series has been seen where **device-local was read correctly five runs
  running**, so "the device heap always fails" is not true either.
- Within a series it is rigid. Ten consecutive runs gave byte-identical
  verdicts; the answer changed only after a rebuild, which is also when the
  figure first moved under the test harness.

So the device heap is unreliable here rather than reliably wrong, which fits a
read of whatever else is resident at that address: stable while the allocation
pattern is, different once something else has moved.

`COLOR_ATTACHMENT` on the image was suspected, since the renderer asks for it
and the reproduction does not. `--attachment` adds it. It changes nothing.

**Whose it is to fix is still not established, and nothing here names a
driver.** What the reproduction buys is the thing the lavapipe one bought:
"our code is contributing" is no longer among the possibilities.

The test harness says the same from its own side: six consecutive runs at
type 0 read 524,288 of 786,432 pixels wrong, and one run immediately after a
rebuild read 581,448.

What the test does about it is take the software device by default, which is the
device the ladder names for this rung anyway and the one CI would have. That
keeps the comparison sensitive to the thing it exists to catch, a fourcc or a
stride meant differently by the two sides.
`EMBLEMA_WRITEBACK_DEVICE=auto` is both the board setting and the one-command
reproduction of the table above.

**No allocation change was made.** Forcing the export into host memory would
make this lane pass, and it was tried -- that is the third row. It is not
shipped, because the only reader it helps is a software controller nobody ships
against, the cost on a part where device-local matters is unmeasured, and a
change justified by one bench lane is the kind this document exists to argue
against.

One more figure from the same runs: at 2560x1600 the writeback fence does not
signal within two seconds, and does within fifteen. A timeout tuned on the small
mode would read as a hang on a large one.

## Which driver the GLES instability is, measured 2026-10-07

`a_blurred_advanced_blend_layer_is_unstable_on_gles` has carried the sentence
"it is the GLES path" since it was reduced. That was concluded against software
*Vulkan*. Comparing against software *GLES* is what nobody had done, and it is
one command:

| driver | device | reduced instability |
|---|---|---|
| `radeonsi` (raphael_mendocino) | workstation | **3 levels** |
| `llvmpipe` (LLVM 22.1.8) | workstation | 0 -- stable |
| Vivante GC7000UL, `V6.4.11.p2.745085` | i.MX8MP | 0 -- stable |
| Adreno 640, OpenGL ES 3.2 | SA8155P | 0 -- stable |

Same backend code, same scene, same call sequence, four drivers, three of them
vendor stacks on real hardware. **`radeonsi` is the only one.** So the sequence
this renderer issues is not sufficient to produce it, and the bare-GLES
reproduction that is still owed has a target and three controls: unstable on
`radeonsi` and clean on the rest, or it is reproducing something else.

**Two drivers is the whole sample, and the reason is worth recording** because
it looks like a gap someone could close and is not:

- **V3D on a Raspberry Pi 5** has no `advanced_blend`, so the test skips. The
  combination needs it twice over.
- **V3D on a Raspberry Pi 5** genuinely has no `advanced_blend`; the test
  skips there and that is the hardware.
- **Vivante GC7000UL and Adreno 640 were unreachable, and that was this
  renderer's doing.** Both run now.

`DisplayTarget` had one variant and it demanded
**`EGL_MESA_platform_surfaceless`**, a Mesa client extension no vendor stack
carries, so the backend refused before asking either board anything. Neither
needs a surface -- both have `EGL_KHR_surfaceless_context`, which is the
*context* extension -- and both ship `libgbm` and advertise
`EGL_KHR_platform_gbm`, which is another way to get a display. A
[`DisplayTarget::Gbm`] variant was added for that, and a request for
surfaceless on a stack without the Mesa platform now falls back to it rather
than failing. Two consequences beyond this investigation: the bench can have
Adreno and Vivante GLES rows for the first time, and `a_context_reports_...`
and the other ten GLES context tests pass on both boards.

**A claim above is wrong and is corrected here.** "There is no GLES device on
that board" was written of the i.MX8MP. There is one, with the right
extension; what is missing is a display target this renderer knows how to ask
for. The shape is the one the writeback connectors had -- a capability
declared absent because of how it was asked for.

## What no machine here checks

`cargo xtask gate` prints what the suite says it covered, under the totals, and
the lines are worth reading together rather than one at a time. They say the
same thing three ways: **advanced blending is the capability this bench cannot
reach.**

On the workstation two of three devices check fourteen of twenty-nine blend
modes against their reference equations, and only the software Vulkan device
reaches all twenty-nine. On a Raspberry Pi 5 it is fourteen on all three. In CI
it is fourteen on all three as well, its lavapipe being a version whose answer
to the question differs from the one here.

That paragraph used to end "so the advanced modes are compared against their
formulas on exactly one device anywhere in this building, and on none in CI",
which is not true and understated the coverage in the direction that matters.
What those counts measure is one of the two ways to reach an advanced mode:
applied as a *batch's* blend, against what is already in the target. That is the
fixed-function path the extension provides, and a device without it refuses the
batch.

The shader's own formulas are reached the other way, by a tint. `drawVertices`
and `drawAtlas` combine a per-vertex or per-sprite color with what the paint
produced, and that happens inside the fragment -- no destination read, no
extension, nothing to gate.
`every_advanced_mode_agrees_with_the_reference_formulas` in
`crates/emblema/tests/tint_blend.rs` checks all fifteen against `emblema_hal`'s
reference, which was written first and independently of the shader, on whatever
device the machine has. It passes on a Raspberry Pi 5, where no device has
advanced blending at all.

So what one device alone reaches is the hardware path rather than the arithmetic.
A transcription error in a blend formula is caught everywhere, including on a
board that refuses every advanced mode as a batch blend. What is thin is the
check that the extension is *driven* correctly -- the blend equation and the
coherency, set per pass -- and that is what the counts above are about.

The scene counts say it again from the other side. Twenty of the catalog's
plates and six of the corpus's need the extension on *both* sides of a
comparison, and no pair here has it: the catalog compares two hundred and
forty-six of two hundred and sixty-six, the corpus fifty-three of fifty-nine,
and those are ceilings rather than shortfalls. A third real GPU would move them;
nothing else on this bench will.

None of that is asserted against, and none of it is a defect -- a device without
an extension cannot exercise it. It is written down because a suite that passes
says nothing about the difference, and because a regression in the advanced
blend arithmetic would be caught today by one machine.

## Where it stands

All fifty-nine test binaries on a Raspberry Pi 5: **901 passed, 0 failed, 0
ignored**, measured 2026-09-22. It was 794 passed and 23 failed the first time the
board was run, and 896 passed with 4 failed immediately before the four entries
above were fixed -- the four are what closed that gap, and the extra test is the
one that came with the filter term.

The Pi 4 is a separate case and is not covered by that number. Its vc4 display
controller refuses to import what this renderer exports, for reasons the DRM
crate's documentation states.

This used to add that the board has no IOMMU "so Vulkan does not come up on it at
all", which is wrong and was wrong in one direction only: the missing IOMMU stops
`vc4` importing `v3d`'s memory, so what a Pi 4 cannot do is *scan out* what Vulkan
allocated. Rendering is fine. `emblema-present-drm`'s crate documentation has
always said so -- "works on a Pi 4 ... what a Pi 4 cannot do is *scan out* what
Vulkan allocated" -- so the tree contradicted itself here for as long as this
paragraph stood.

Measured 2026-09-17, kernel 6.18.34+rpt-rpi-v8 on Debian 13: `cargo xtask bench`
reports `vulkan:0 V3D 4.2.14.0` beside `gles V3D 4.2.14.0` and both give all four
rows. So a Vulkan *rendering* failure on that board is a bug like any other, which
is the opposite of what a reader was being told to conclude.

Two things it is worth knowing before benching there. The board is about four
times slower than a Pi 5 -- 34 ms against 8.9 for the distance-field row -- and
its bimodality is the Pi 5's: two runs of the same binary came back 34.386 and
33.668 ms, two per cent apart, so the three-runs-a-side rule applies unchanged.
The cached Pi 5 sysroot links binaries that run on it without alteration; both
boards are glibc 2.41 and gcc 14.
