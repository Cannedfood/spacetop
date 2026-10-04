use super::*;

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

    pub unsafe fn draw<'a>(
        &self,
        command: vk::CommandBuffer,
        target: &RenderTarget,
        view: &xr::View,
        frame: &SceneFrame<'a>,
    ) {
        let projection = view_projection(view);
        let mut panels = frame.panels.to_vec();
        panels.sort_by(|(_, first), (_, second)| {
            let first = (projection * first.pose.center.extend(1.0)).w;
            let second = (projection * second.pose.center.extend(1.0)).w;
            second.total_cmp(&first)
        });
        let area = vk::Rect2D::default().extent(target.extent);
        let clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        unsafe {
            self.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(target.framebuffer)
                    .render_area(area)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            self.device.cmd_set_viewport(
                command,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: target.extent.width as f32,
                    height: target.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            self.device.cmd_set_scissor(command, 0, &[area]);
            self.device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                1,
                &[self.floor_descriptor, self.environment_descriptor],
                &[],
            );
            let floor_height = [frame.floor_y];
            let floor_height_bytes =
                std::slice::from_raw_parts(floor_height.as_ptr().cast::<u8>(), 4);
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                108,
                floor_height_bytes,
            );
            let mut eye = [
                view.pose.position.x,
                view.pose.position.y,
                view.pose.position.z,
                0.0,
            ];
            let eye_bytes = std::slice::from_raw_parts(eye.as_ptr().cast::<u8>(), 16);
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                112,
                eye_bytes,
            );
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.environment_pipelines[usize::from(self.ambient_occlusion) * 2
                    + usize::from(self.trace_through_transparent_windows)],
            );
            self.draw_panel(command, sky_matrix(view));
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.window_pipeline,
            );
            for (texture, geometry) in panels {
                let close_hit = frame
                    .cursor_close_panel
                    .filter(|(close_geometry, _)| *close_geometry == geometry);
                let mut cursor_position = close_hit.map_or([0.0; 4], |(_, position)| {
                    [
                        position.x + self.window_padding_px,
                        position.y + self.window_padding_px,
                        0.0,
                        0.0,
                    ]
                });
                cursor_position[2] = frame.texture_sample_phase as f32;
                let cursor_position_bytes =
                    std::slice::from_raw_parts(cursor_position.as_ptr().cast::<u8>(), 16);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    64,
                    cursor_position_bytes,
                );
                eye[0] = geometry.logical_size.w as f32
                    + self.window_padding_px * 2.0
                    + self.max_border_width_px
                    + 2.0;
                eye[1] = geometry.logical_size.h as f32
                    + self.window_padding_px * 2.0
                    + self.max_border_width_px
                    + 2.0;
                eye[2] = if frame.grabbed_panel == Some(geometry) {
                    1.0
                } else {
                    0.0
                };
                eye[3] = if close_hit.is_some() { 1.0 } else { 0.0 };
                let eye_bytes = std::slice::from_raw_parts(eye.as_ptr().cast::<u8>(), 16);
                self.device.cmd_push_constants(
                    command,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    112,
                    eye_bytes,
                );
                self.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[texture.descriptor],
                    &[],
                );
                self.draw_panel(
                    command,
                    projection
                        * expanded_window_model(
                            geometry,
                            self.window_padding_px,
                            self.max_border_width_px,
                        ),
                );
            }
            if let Some(mut pose) = frame.cursor {
                pose.center += pose.orientation() * Vec3::Z * 0.001;
                self.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.cursor_pipeline,
                );
                self.draw_panel(
                    command,
                    projection
                        * model(PanelGeometry {
                            pose,
                            logical_size: (21, 21).into(),
                        }),
                );
            }
            self.device.cmd_end_render_pass(command);
        }
    }

    unsafe fn draw_panel(&self, command: vk::CommandBuffer, matrix: Mat4) {
        let columns = matrix.to_cols_array();
        let bytes = unsafe { std::slice::from_raw_parts(columns.as_ptr().cast::<u8>(), 64) };
        unsafe {
            self.device.cmd_push_constants(
                command,
                self.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes,
            );
            self.device.cmd_draw(command, 6, 1, 0, 0);
        }
    }
}
