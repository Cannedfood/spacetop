struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    let uv = corners[index];
    var result: Vertex;
    result.position = transform.matrix * vec4(uv.x - 0.5, 0.5 - uv.y, 0.0, 1.0);
    result.uv = uv;
    return result;
}
fn rounded_box_distance(point: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let half_size = size * 0.5;
    let q = abs(point - half_size) - (half_size - vec2(radius));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}
fn largest_border_width() -> f32 {
    return max(
        max(floor_material.window_style.y, floor_material.window_style.w),
        floor_material.grabbed_style.x,
    );
}
fn sdf_coverage(distance: f32) -> f32 {
    let pixel_width = max(fwidth(distance), 0.0001);
    return 1.0 - smoothstep(-pixel_width * 0.5, pixel_width * 0.5, distance);
}
const PATTERN_4: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2(-0.177, -0.306),
    vec2(0.306, -0.177),
    vec2(0.177, 0.306),
    vec2(-0.306, 0.177),
);
const PATTERN_8: array<vec2<f32>, 8> = array<vec2<f32>, 8>(
    vec2(-0.265165, -0.459279),
    vec2(0.217798, -0.329870),
    vec2(-0.088388, -0.153093),
    vec2(-0.394575, 0.023684),
    vec2(0.394575, -0.023684),
    vec2(0.088388, 0.153093),
    vec2(-0.217798, 0.329870),
    vec2(0.265165, 0.459279),
);
const PATTERN_16: array<vec2<f32>, 16> = array<vec2<f32>, 16>(
    vec2(-0.265165, -0.459279),
    vec2(-0.023684, -0.394575),
    vec2(0.217798, -0.329870),
    vec2(0.459279, -0.265165),
    vec2(0.394575, -0.023684),
    vec2(0.153093, -0.088388),
    vec2(-0.088388, -0.153093),
    vec2(-0.329870, -0.217798),
    vec2(-0.394575, 0.023684),
    vec2(-0.153093, 0.088388),
    vec2(0.088388, 0.153093),
    vec2(0.329870, 0.217798),
    vec2(0.265165, 0.459279),
    vec2(0.023684, 0.394575),
    vec2(-0.217798, 0.329870),
    vec2(-0.459279, 0.265165),
);
fn sample_panel(uv: vec2<f32>) -> vec4<f32> {
    let derivatives_x = dpdx(uv);
    let derivatives_y = dpdy(uv);
    let mode = u32(floor(floor_material.sampling.z + 0.5));
    if mode == 0u {
        return textureSampleGrad(panel, filtering, uv, derivatives_x, derivatives_y);
    }

    var pattern_count = 4u;
    if mode >= 3u && mode <= 4u {
        pattern_count = 8u;
    } else if mode >= 5u {
        pattern_count = 16u;
    }
    let temporal = mode == 1u || mode == 3u || mode == 5u;
    let phase = select(0u, u32(transform.emitter_center_width.z + 0.5), temporal);
    let sample_count = select(pattern_count, pattern_count / 2u, temporal);
    var color = vec4(0.0);
    for (var index = 0u; index < pattern_count; index += 1u) {
        if temporal && index % 2u != phase {
            continue;
        }
        var offset = PATTERN_4[index % 4u];
        if pattern_count == 8u {
            offset = PATTERN_8[index];
        } else if pattern_count == 16u {
            offset = PATTERN_16[index];
        }
        color += textureSampleGrad(
            panel,
            filtering,
            uv + derivatives_x * offset.x + derivatives_y * offset.y,
            derivatives_x,
            derivatives_y,
        );
    }
    return color / f32(sample_count);
}
@fragment fn window(input: Vertex) -> @location(0) vec4<f32> {
    let expanded_size = max(transform.eye_position.xy, vec2(1.0));
    let largest_border = largest_border_width();
    let size = max(expanded_size - vec2(largest_border + 2.0), vec2(1.0));
    let point = input.uv * expanded_size - vec2(largest_border * 0.5 + 1.0);
    let radius = min(floor_material.window_style.z, min(size.x, size.y) * 0.5);
    let outer_distance = rounded_box_distance(point, size, radius);

    let close_distance = length(point - transform.emitter_center_width.xy);
    let proximity_radius = max(floor_material.grabbed_style.y, 0.001);
    let close_amount = select(
        0.0,
        1.0 - smoothstep(0.0, proximity_radius, close_distance),
        transform.eye_position.w > 0.5 && floor_material.grabbed_style.y > 0.0,
    );
    let grabbed = transform.eye_position.z > 0.5;
    var border_width = mix(
        floor_material.window_style.y,
        floor_material.window_style.w,
        close_amount,
    );
    var border_color = mix(
        floor_material.border_color,
        floor_material.cursor_close_border_color,
        close_amount,
    );
    if grabbed {
        border_width = floor_material.grabbed_style.x;
        border_color = floor_material.grabbed_border_color;
    }

    let padding = max(floor_material.window_style.x, largest_border * 0.5);
    let content_size = max(size - vec2(2.0 * padding), vec2(1.0));
    let content_point = point - vec2(padding);
    let content_radius = max(radius - padding, 0.0);
    let content_distance = rounded_box_distance(content_point, content_size, content_radius);
    let outer_coverage = sdf_coverage(outer_distance);
    let inner_coverage = sdf_coverage(outer_distance + border_width * 0.5);
    let border_coverage = max(outer_coverage - inner_coverage, 0.0);
    let content_coverage = inner_coverage * sdf_coverage(content_distance);
    if border_coverage + content_coverage < 0.001 { discard; }

    let color = sample_panel(content_point / content_size);
    let content_alpha = color.a * content_coverage;
    let border_alpha = border_color.a * border_coverage;
    return vec4(
        color.rgb * content_coverage + border_color.rgb * border_alpha,
        content_alpha + border_alpha,
    );
}
@fragment fn cursor(input: Vertex) -> @location(0) vec4<f32> {
    let stroke = max(vec2(0.5 / 21.0), fwidth(input.uv) * 0.75);
    if all(abs(input.uv - vec2(0.5)) > stroke) { discard; }
    return vec4(1.0, 0.9131, 0.0, 1.0);
}
