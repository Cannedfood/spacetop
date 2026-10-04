const TRACE_THROUGH_TRANSPARENT_WINDOWS: bool = false;
const AMBIENT_OCCLUSION: bool = false;

struct SkyVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) direction: vec3<f32>,
}
@vertex fn sky_vertex(@builtin(vertex_index) index: u32) -> SkyVertex {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0),
        vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 1.0));
    let uv = corners[index];
    var result: SkyVertex;
    result.position = vec4(uv * 2.0 - 1.0, 1.0, 1.0);
    result.direction = (transform.matrix * vec4(uv, 0.0, 1.0)).xyz;
    return result;
}
fn skybox_exposure() -> f32 {
    return exp2(floor_material.controls.y) * floor_material.sampling.w;
}
fn dim_environment(color: vec3<f32>) -> vec3<f32> {
    return color * (1.0 - floor_material.ground_radius.z);
}
fn skybox_uv(direction: vec3<f32>) -> vec2<f32> {
    let longitude = atan2(direction.z, direction.x) + floor_material.sampling.y;
    return vec2(
        fract(longitude / (2.0 * PI) + 0.5),
        0.5 - asin(clamp(direction.y, -1.0, 1.0)) / PI);
}
fn skybox_mip_level(
    direction: vec3<f32>,
    half_vector: vec3<f32>,
    view: vec3<f32>,
    alpha: f32,
    sample_count: u32,
) -> f32 {
    let n_dot_v = max(view.y, 0.00001);
    let n_dot_h = max(half_vector.y, 0.00001);
    let alpha_squared = alpha * alpha;
    let distribution_denominator = n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
    let distribution = alpha_squared
        / (PI * distribution_denominator * distribution_denominator);
    let direction_pdf = distribution * ggx_masking(n_dot_v, alpha_squared) / (4.0 * n_dot_v);
    let sample_solid_angle = 1.0 / (f32(sample_count) * max(direction_pdf, 0.0000001));
    let dimensions = vec2<f32>(textureDimensions(environment_skybox, 0));
    let latitude_cosine = sqrt(max(1.0 - direction.y * direction.y, 0.000001));
    let texel_solid_angle = (2.0 * PI / dimensions.x) * (PI / dimensions.y) * latitude_cosine;
    let mip_level = 0.5 * log2(sample_solid_angle / texel_solid_angle);
    return clamp(mip_level, 0.0, f32(textureNumLevels(environment_skybox) - 1u));
}
fn sample_environment_skybox(direction: vec3<f32>, mip_level: f32) -> vec3<f32> {
    let uv = skybox_uv(direction);
    let hdr = max(
        textureSampleLevel(environment_skybox, environment_sky_filter, uv, mip_level).rgb
            * skybox_exposure(),
        vec3(0.0),
    );
    return reinhard_tone_map(hdr);
}
fn nearest_window_hit(origin: vec3<f32>, ray: vec3<f32>, minimum_distance: f32) -> WindowHit {
    var nearest = WindowHit(1.0e30, 0u, vec2(0.0), false);
    let count = window_buffer.count;
    for (var index = 0u; index < count; index += 1u) {
        let window = window_buffer.windows[index];
        let center = window.center_width.xyz;
        let width = window.center_width.w;
        let right = window.right_height.xyz;
        let height = window.right_height.w;
        let up = window.up.xyz;
        let normal = cross(right, up);
        let denominator = dot(ray, normal);
        if denominator >= -0.00001 { continue; }
        let distance = dot(center - origin, normal) / denominator;
        if distance <= minimum_distance || distance >= nearest.distance { continue; }
        let hit = origin + ray * distance - center;
        let uv = vec2(dot(hit, right) / width + 0.5, 0.5 - dot(hit, up) / height);
        if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) { continue; }
        nearest = WindowHit(distance, index, uv, true);
    }
    return nearest;
}
fn sample_reflected_window(index: u32, uv: vec2<f32>) -> vec4<f32> {
    let rect = window_buffer.windows[index].atlas_rect;
    let pixel = clamp(
        rect.xy + uv * rect.zw,
        rect.xy + vec2(0.5),
        rect.xy + rect.zw - vec2(0.5),
    );
    let atlas_dimensions = vec2<f32>(textureDimensions(panel_atlas));
    return textureSampleLevel(
        panel_atlas,
        environment_filter,
        pixel / atlas_dimensions,
        0.0,
    );
}
fn sample_reflected_environment(origin: vec3<f32>, ray: vec3<f32>, mip_level: f32) -> vec3<f32> {
    if TRACE_THROUGH_TRANSPARENT_WINDOWS {
        var radiance = vec3(0.0);
        var remaining = 1.0;
        var minimum_distance = 0.00001;
        for (var layer = 0u; layer < window_buffer.count; layer += 1u) {
            let hit = nearest_window_hit(origin, ray, minimum_distance);
            if !hit.found { break; }
            let color =
                sample_reflected_window(hit.index, hit.uv);
            radiance += color.rgb * remaining;
            remaining *= 1.0 - color.a;
            if remaining < 0.001 { return radiance; }
            minimum_distance = hit.distance + 0.00001;
        }
        if remaining > 0.001 {
            radiance += sample_environment_skybox(ray, mip_level) * remaining;
        }
        return radiance;
    }
    let hit = nearest_window_hit(origin, ray, 0.00001);
    if hit.found {
        return sample_reflected_window(hit.index, hit.uv).rgb;
    }
    return sample_environment_skybox(ray, mip_level);
}
// Returns a truncated Cauchy sample and its inverse PDF in window-space meters.
fn sample_window_offset(bounds: vec2<f32>, scale: f32, random: f32) -> vec2<f32> {
    let angle_span = atan2(
        scale * (bounds.y - bounds.x),
        scale * scale + bounds.x * bounds.y,
    );
    let angle = random * angle_span;
    let sine = sin(angle);
    let cosine = cos(angle);
    let offset = clamp(
        scale * (bounds.x * cosine + scale * sine)
            / (scale * cosine - bounds.x * sine),
        bounds.x,
        bounds.y,
    );
    let inverse_pdf = angle_span * (scale * scale + offset * offset) / scale;
    return vec2(offset, inverse_pdf);
}
fn window_blocked_diffuse_irradiance(origin: vec3<f32>) -> vec3<f32> {
    var blocked = vec3(0.0);
    let base_dimensions = textureDimensions(environment_skybox, 0u);
    let mip_count = textureNumLevels(environment_skybox);
    for (var index = 0u; index < window_buffer.count; index += 1u) {
        let window = window_buffer.windows[index];
        let center = window.center_width.xyz;
        let width = window.center_width.w;
        let right = window.right_height.xyz;
        let height = window.right_height.w;
        let up = window.up.xyz;

        let center_to_window = center - origin;
        let center_distance_squared = dot(center_to_window, center_to_window);
        if center_distance_squared <= 0.0001 { continue; }
        let reference_u = skybox_uv(normalize(center_to_window)).x;
        var window_uv_min = vec2(1.0e30);
        var window_uv_max = vec2(-1.0e30);
        var crosses_pole = false;
        for (var corner_index = 0u; corner_index < 4u; corner_index += 1u) {
            let horizontal = select(-0.5, 0.5, (corner_index & 1u) != 0u);
            let vertical = select(-0.5, 0.5, (corner_index & 2u) != 0u);
            let corner = center + right * horizontal * width + up * vertical * height;
            let to_corner = corner - origin;
            if dot(to_corner, to_corner) <= 0.0001 {
                crosses_pole = true;
                break;
            }
            let corner_uv = skybox_uv(normalize(to_corner));
            var delta_u = corner_uv.x - reference_u;
            if delta_u > 0.5 { delta_u -= 1.0; }
            if delta_u < -0.5 { delta_u += 1.0; }
            let unwrapped_u = reference_u + delta_u;
            window_uv_min = min(window_uv_min, vec2(unwrapped_u, corner_uv.y));
            window_uv_max = max(window_uv_max, vec2(unwrapped_u, corner_uv.y));
        }
        if crosses_pole { continue; }
        let window_uv_center = (window_uv_min + window_uv_max) * 0.5;
        let sky_uv = vec2(fract(window_uv_center.x), window_uv_center.y);
        let window_texel_width =
            (window_uv_max.x - window_uv_min.x) * f32(base_dimensions.x);
        let window_texel_height =
            (window_uv_max.y - window_uv_min.y) * f32(base_dimensions.y);
        var coarsest_two_row_mip = 0u;
        for (var candidate_mip = 0u; candidate_mip < mip_count; candidate_mip += 1u) {
            if textureDimensions(environment_skybox, candidate_mip).y < 2u { break; }
            coarsest_two_row_mip = candidate_mip;
        }
        let mip_level = min(
            max(log2(max(max(window_texel_width, window_texel_height), 1.0)), 0.0),
            f32(coarsest_two_row_mip),
        );
        let jitter = ao_sample_jitter(origin, index);
        let normal = normalize(cross(right, up));
        let plane_distance = abs(dot(center_to_window, normal));
        var offset = vec2((jitter.x - 0.5) * width, (0.5 - jitter.y) * height);
        var inverse_area_pdf = width * height;
        // Keep uniform sampling beyond a window diagonal, where distance weighting is weak.
        if plane_distance * plane_distance < width * width + height * height {
            let projected_center = vec2(dot(center_to_window, right), dot(center_to_window, up));
            let horizontal_bounds = projected_center.x + vec2(-0.5, 0.5) * width;
            let vertical_bounds = projected_center.y + vec2(-0.5, 0.5) * height;
            let nearest_vertical = clamp(0.0, vertical_bounds.x, vertical_bounds.y);
            // Bias toward nearby surface area; regularize only the proposal, not the geometry.
            let horizontal_scale = max(
                sqrt(plane_distance * plane_distance + nearest_vertical * nearest_vertical),
                0.0001,
            );
            let horizontal_sample =
                sample_window_offset(horizontal_bounds, horizontal_scale, jitter.x);
            let vertical_scale = max(
                sqrt(plane_distance * plane_distance + horizontal_sample.x * horizontal_sample.x),
                0.0001,
            );
            let vertical_sample = sample_window_offset(vertical_bounds, vertical_scale, jitter.y);
            offset = vec2(horizontal_sample.x, vertical_sample.x) - projected_center;
            inverse_area_pdf = horizontal_sample.y * vertical_sample.y;
        }
        let window_uv = vec2(offset.x / width + 0.5, 0.5 - offset.y / height);
        let point_on_window = center
            + right * offset.x
            + up * offset.y;
        let to_window_sample = point_on_window - origin;
        let distance_squared = dot(to_window_sample, to_window_sample);
        if distance_squared <= 0.0001 { continue; }
        let direction_to_window_sample = to_window_sample * inverseSqrt(distance_squared);
        if direction_to_window_sample.y <= 0.0 { continue; }

        let window_cosine = abs(dot(normal, direction_to_window_sample));
        if window_cosine <= 0.00001 { continue; }

        // Divide by the joint area PDF instead of multiplying by uniform window area.
        let window_cosine_weighted_area =
            inverse_area_pdf * window_cosine * direction_to_window_sample.y / distance_squared;
        let radiance = skybox_exposure() * textureSampleLevel(
            environment_skybox,
            environment_sky_filter,
            sky_uv,
            mip_level,
        ).rgb;
        blocked += radiance * window_cosine_weighted_area / PI;
    }
    return blocked;
}
@fragment fn environment(input: SkyVertex) -> @location(0) vec4<f32> {
    let eye = transform.eye_position.xyz;
    let incident = normalize(input.direction);
    if incident.y >= 0.0 {
        return vec4(dim_environment(sample_environment_skybox(incident, 0.0)), 1.0);
    }
    let floor_distance = (transform.emitter_up.w - eye.y) / incident.y;
    if floor_distance <= 0.0 {
        return vec4(dim_environment(sample_environment_skybox(incident, 0.0)), 1.0);
    }
    let world = eye + incident * floor_distance;
    let ground_distance_from_origin = length(world.xz);
    let height_from_origin = abs(transform.emitter_up.w);
    let ground_angle_degrees =
        atan2(ground_distance_from_origin, height_from_origin) * (180.0 / PI);
    let radius_degrees = floor_material.ground_radius.x;
    if ground_angle_degrees >= radius_degrees {
        return vec4(dim_environment(sample_environment_skybox(incident, 0.0)), 1.0);
    }
    var ground_coverage = 1.0;
    if radius_degrees < 90.0 && floor_material.ground_radius.y > 0.0 {
        let radius_radians = radius_degrees * PI / 180.0;
        let floor_radius_m = height_from_origin * sin(radius_radians)
            / max(cos(radius_radians), 0.000001);
        if ground_distance_from_origin >= floor_radius_m {
            return vec4(dim_environment(sample_environment_skybox(incident, 0.0)), 1.0);
        }
        let feather_start_m = max(0.0, floor_radius_m - floor_material.ground_radius.y);
        ground_coverage = 1.0 - smoothstep(
            feather_start_m,
            floor_radius_m,
            ground_distance_from_origin,
        );
    }
    let view = -incident;
    let roughness = floor_material.controls.z;
    let ray_count = u32(floor_material.controls.w);
    let alpha = max(0.001, roughness * roughness);
    let alpha_squared = alpha * alpha;
    let view_masking = ggx_masking(view.y, alpha_squared);
    let fresnel = fresnel_schlick(-incident.y);
    let opacity = floor_material.albedo.a;
    var visible_diffuse_irradiance = floor_material.diffuse_irradiance.rgb;
    if AMBIENT_OCCLUSION {
        visible_diffuse_irradiance = max(
            visible_diffuse_irradiance - window_blocked_diffuse_irradiance(world),
            vec3(0.0),
        );
    }
    var ground = floor_material.albedo.rgb * visible_diffuse_irradiance
        * (1.0 - fresnel) * opacity;
    if opacity < 1.0 {
        ground += sample_environment_skybox(incident, 0.0) * (1.0 - opacity);
    }
    let rows = max(1u, u32(sqrt(f32(ray_count))));
    let short_row_count = ray_count / rows;
    let long_rows = ray_count % rows;
    var sum = vec3(0.0);
    for (var index = 0u; index < ray_count; index += 1u) {
        let row = index % rows;
        let columns = short_row_count + select(0u, 1u, row < long_rows);
        let row_offset = row * short_row_count + min(row, long_rows);
        let jitter = sample_jitter(world, index);
        let sample_uv = vec2(
            (f32(index / rows) + jitter.x) / f32(columns),
            (f32(row_offset) + jitter.y * f32(columns)) / f32(ray_count));
        let half_vector = sample_ggx_visible_normal(view, sample_uv, alpha);
        let ray = reflect(-view, half_vector);
        if ray.y <= 0.0 { continue; }
        let mip_level = skybox_mip_level(ray, half_vector, view, alpha, ray_count);
        let radiance = sample_reflected_environment(world, ray, mip_level);
        let view_half = clamp(dot(view, half_vector), 0.0, 1.0);
        let sample_fresnel = fresnel_schlick(view_half);
        let light_masking = ggx_masking(ray.y, alpha_squared);
        let weight = sample_fresnel * light_masking
            / (view_masking + light_masking - view_masking * light_masking);
        sum += radiance * weight;
    }
    let sky = sample_environment_skybox(incident, 0.0);
    let floor = ground + sum * opacity / f32(ray_count);
    return vec4(dim_environment(mix(sky, floor, ground_coverage)), 1.0);
}
