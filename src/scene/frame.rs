use ash::vk;
use glam::{Mat4, Vec3};
use openxr as xr;

use super::skybox::sky_matrix;
use super::{
    RenderTarget, SceneFrame, SceneRenderer, expanded_window_model, model, view_projection,
};
use crate::panel::PanelGeometry;

impl SceneRenderer {
    pub unsafe fn copy_reflection_textures(
        &mut self,
        command: vk::CommandBuffer,
        frame: &SceneFrame<'_>,
    ) {
        unsafe {
            self.atlas
                .copy_textures(&self.context.device, command, frame.panels);
        }
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
                &[self.floor.descriptor, self.environment.descriptor],
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
                self.environment_pipelines[usize::from(self.window.ambient_occlusion) * 2
                    + usize::from(self.window.trace_through_transparent_windows)],
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
                        position.x + self.window.window_padding_px,
                        position.y + self.window.window_padding_px,
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
                    + self.window.window_padding_px * 2.0
                    + self.window.max_border_width_px
                    + 2.0;
                eye[1] = geometry.logical_size.h as f32
                    + self.window.window_padding_px * 2.0
                    + self.window.max_border_width_px
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
                            self.window.window_padding_px,
                            self.window.max_border_width_px,
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
