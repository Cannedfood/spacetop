use anyhow::{Context, Result};
use ash::vk;

use super::SceneRenderer;

const SHADER_PARTS: [&str; 5] = [
    include_str!("../shaders/shared.wgsl"),
    include_str!("../shaders/panel.wgsl"),
    include_str!("../shaders/tone_mapping.wgsl"),
    include_str!("../shaders/lighting.wgsl"),
    include_str!("../shaders/environment.wgsl"),
];
fn shader(
    entry: &str,
    stage: naga::ShaderStage,
    trace_through_transparent_windows: bool,
    ambient_occlusion: bool,
) -> Result<Vec<u32>> {
    let mut shader_source = SHADER_PARTS.join("\n").replace(
        "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = false;",
        &format!(
            "const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = {};",
            trace_through_transparent_windows
        ),
    );
    shader_source = shader_source.replace(
        "const AMBIENT_OCCLUSION: bool = false;",
        &format!("const AMBIENT_OCCLUSION: bool = {ambient_occlusion};"),
    );
    let module = naga::front::wgsl::parse_str(&shader_source)
        .map_err(|error| anyhow::anyhow!(error.emit_to_string(&shader_source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::IMMEDIATES,
    )
    .validate(&module)?;
    let options = naga::back::spv::Options {
        flags: naga::back::spv::WriterFlags::empty(),
        ..Default::default()
    };
    Ok(naga::back::spv::write_vec(
        &module,
        &info,
        &options,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.into(),
        }),
    )?)
}

impl SceneRenderer {
    pub(super) fn pipeline(
        &self,
        fragment: &str,
        trace_through_transparent_windows: bool,
        ambient_occlusion: bool,
    ) -> Result<vk::Pipeline> {
        let vertex_name = if fragment == "environment" {
            c"sky_vertex"
        } else {
            c"vertex"
        };
        let vertex_code = shader(
            vertex_name.to_str()?,
            naga::ShaderStage::Vertex,
            trace_through_transparent_windows,
            ambient_occlusion,
        )?;
        let fragment_code = shader(
            fragment,
            naga::ShaderStage::Fragment,
            trace_through_transparent_windows,
            ambient_occlusion,
        )?;
        let vertex = unsafe {
            self.device.create_shader_module(
                &vk::ShaderModuleCreateInfo::default().code(&vertex_code),
                None,
            )
        }?;
        let result = (|| -> Result<_> {
            let fragment_module = unsafe {
                self.device.create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&fragment_code),
                    None,
                )
            }?;
            let fragment_name = std::ffi::CString::new(fragment)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vertex)
                    .name(vertex_name),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(fragment_module)
                    .name(&fragment_name),
            ];
            let input = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let samples = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(fragment != "environment")
                .depth_write_enable(fragment == "window")
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            let alpha_blend = fragment == "floor_albedo";
            let attachments = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(fragment != "environment")
                .src_color_blend_factor(if alpha_blend {
                    vk::BlendFactor::SRC_ALPHA
                } else {
                    vk::BlendFactor::ONE
                })
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
            let dynamic = vk::PipelineDynamicStateCreateInfo::default()
                .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&samples)
                .depth_stencil_state(&depth)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(self.layout)
                .render_pass(self.render_pass);
            let result = unsafe {
                self.device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            };
            unsafe {
                self.device.destroy_shader_module(fragment_module, None);
            }
            match result {
                Ok(pipelines) => Ok(pipelines[0]),
                Err((pipelines, error)) => {
                    for pipeline in pipelines {
                        unsafe {
                            self.device.destroy_pipeline(pipeline, None);
                        }
                    }
                    Err(error.into())
                }
            }
        })();
        unsafe {
            self.device.destroy_shader_module(vertex, None);
        }
        result.context("create Vulkan scene pipeline")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_compile() {
        for (entry, stage) in [
            ("sky_vertex", naga::ShaderStage::Vertex),
            ("vertex", naga::ShaderStage::Vertex),
            ("window", naga::ShaderStage::Fragment),
            ("cursor", naga::ShaderStage::Fragment),
            ("environment", naga::ShaderStage::Fragment),
        ] {
            for ambient_occlusion in [false, true] {
                assert_eq!(
                    shader(entry, stage, false, ambient_occlusion).unwrap()[0],
                    0x0723_0203
                );
            }
            for trace_transparent in [false, true] {
                for ambient_occlusion in [false, true] {
                    assert_eq!(
                        shader(
                            "environment",
                            naga::ShaderStage::Fragment,
                            trace_transparent,
                            ambient_occlusion,
                        )
                        .unwrap()[0],
                        0x0723_0203
                    );
                }
            }
        }
        assert_eq!(
            shader("environment", naga::ShaderStage::Fragment, true, false).unwrap()[0],
            0x0723_0203
        );
    }
}
