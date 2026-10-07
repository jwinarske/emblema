//! Shader sources and the artifacts built from them.
//!
//! One WGSL source tree is the single source of truth. `build.rs` runs naga
//! over it to produce SPIR-V for Vulkan, and can emit GLSL, MSL, and HLSL for
//! backends that do not exist yet, from the same source and with no C++ shader
//! toolchain anywhere in the build.
//!
//! Translation happens at build time, so nothing here parses shaders at
//! runtime and no shader compiler ships in the binary. That is what makes
//! "all pipelines compiled ahead of time, no compilation jank" achievable
//! rather than aspirational.

include!(concat!(env!("OUT_DIR"), "/shaders.rs"));

#[cfg(test)]
mod tests {
    #[test]
    fn every_target_is_generated_from_the_same_source() {
        // A shader that translated to one target and silently not the other
        // would leave a backend rendering something stale. Both are generated
        // from one source in the same build step, and both must be present.
        assert!(super::SOLID_VS_GLSL.contains("#version 300 es"));
        assert!(super::SOLID_FS_GLSL.contains("#version 300 es"));
        assert!(!super::SOLID_SPV.is_empty());
    }

    #[test]
    fn the_glsl_carries_the_paint_block_under_the_name_the_backend_looks_up() {
        // The GLES backend asks for this block by name and assigns its binding
        // point, so the name is part of the contract rather than an
        // implementation detail. It was `_push_constant_binding_fs` while the
        // paint traveled as a push constant, and this test is what said so.
        assert!(
            super::SOLID_FS_GLSL.contains("uniform Paint_block_0Fragment"),
            "the paint block is missing or renamed:\n{}",
            super::SOLID_FS_GLSL
        );
    }

    #[test]
    fn the_paint_block_states_the_layout_rather_than_taking_the_default() {
        // A uniform block with no qualifier is `shared`, whose member offsets
        // the implementation chooses and a caller is expected to ask for. Both
        // backends write the bytes themselves, so the layout has to be the one
        // every implementation agrees on. The translator will not write the
        // qualifier for GLSL ES 3.00, so the build step adds it -- and this is
        // what fails if that ever stops happening.
        assert!(
            super::SOLID_FS_GLSL.contains("layout(std140) uniform Paint_block_"),
            "the paint block would be laid out at the driver's discretion:\n{}",
            super::SOLID_FS_GLSL
        );
    }

    #[test]
    fn both_targets_adjust_clip_space_the_same_way() {
        // The vertex stage negates Y for both targets, which is what makes one
        // WGSL source produce matching orientation on backends whose
        // framebuffer origins disagree. If only one target adjusted, output
        // would be mirrored on the other.
        assert!(
            super::SOLID_VS_GLSL.contains("-gl_Position.y"),
            "the GLSL stage does not adjust clip space:\n{}",
            super::SOLID_VS_GLSL
        );
    }

    #[test]
    fn spirv_is_well_formed() {
        // The SPIR-V magic number. A mismatch means the generator emitted
        // something other than a module, or emitted it byte-swapped.
        assert_eq!(super::SOLID_SPV.first().copied(), Some(0x0723_0203));
        // Header is five words; anything at or below that is an empty module.
        assert!(super::SOLID_SPV.len() > 5, "module is header-only");
    }

    /// No module builds a vector out of a shader function's parameter.
    ///
    /// A driver workaround with teeth. Vivante's SPIR-V decoder segfaults
    /// inside `vkCreate*Pipelines` on an `OpCompositeConstruct`, in a
    /// non-entry-point function, one of whose operands is a value parameter or
    /// a component extracted from one. `docs/on-a-board.md` has the rule and
    /// the probe that established it; `solid.wgsl` carries a `var` at each of
    /// the seven places it used to happen.
    ///
    /// Those `var`s look like noise and will be tidied away by someone who
    /// does not know, which is what this is for. It reads the built SPIR-V
    /// rather than the WGSL, because what matters is what naga emitted.
    #[test]
    fn the_shader_builds_no_vector_from_a_parameter() {
        for (name, words) in super::MODULES {
            let mut entries = Vec::new();
            let mut derived = Vec::new();
            let mut current = 0u32;
            let mut found = Vec::new();
            let mut i = 5;
            while i < words.len() {
                let (count, op) = ((words[i] >> 16) as usize, words[i] & 0xFFFF);
                if count == 0 {
                    break;
                }
                match op {
                    // OpEntryPoint, OpFunction, OpFunctionParameter
                    15 => entries.push(words[i + 2]),
                    54 => current = words[i + 2],
                    55 => derived.push(words[i + 2]),
                    // Anything that carries a parameter's value forward:
                    // extract, shuffle, access chain.
                    81 | 79 | 65 | 66 if count >= 4 => {
                        if words[i + 3..i + count].iter().any(|o| derived.contains(o)) {
                            derived.push(words[i + 2]);
                        }
                    }
                    // OpCompositeConstruct, outside an entry point
                    80 if count >= 4 && !entries.contains(&current) => {
                        if words[i + 3..i + count].iter().any(|o| derived.contains(o)) {
                            found.push(words[i + 2]);
                        }
                    }
                    _ => {}
                }
                i += count;
            }
            assert!(
                found.is_empty(),
                "{name} builds {} vector(s) from a function parameter, which \
                 segfaults one driver's pipeline compiler. Put the operand in \
                 a `var` first, as the rest of {name} does.",
                found.len()
            );
        }
    }
}
