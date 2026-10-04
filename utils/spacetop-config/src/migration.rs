use anyhow::{Context, Result, bail, ensure};

use super::{CONFIG_VERSION_KEY, CURRENT_CONFIG_VERSION, FloorConfig};

pub(super) fn migrate_config(value: &mut toml::Value) -> Result<()> {
    let mut version = match value
        .as_table_mut()
        .context("configuration root must be a TOML table")?
        .remove(CONFIG_VERSION_KEY)
    {
        Some(toml::Value::Integer(version)) => version,
        Some(_) => bail!("config_version must be an integer"),
        None => 0,
    };

    ensure!(version >= 0, "config_version cannot be negative");
    ensure!(
        version <= CURRENT_CONFIG_VERSION,
        "configuration version {version} is newer than supported version {CURRENT_CONFIG_VERSION}"
    );

    if version < 1 {
        version = 1;

        let root = value
            .as_table_mut()
            .context("configuration root must be a TOML table")?;

        if let Some(floor) = root.get_mut("floor").and_then(toml::Value::as_table_mut) {
            let transparency = floor
                .remove("transparency")
                .map(|value| value_as_f32(&value, "floor.transparency"))
                .transpose()?;

            match floor.get_mut("albedo") {
                Some(value) => {
                    let channels = value
                        .as_array_mut()
                        .context("floor.albedo must contain three legacy or four RGBA channels")?;
                    match channels.len() {
                        3 => channels.push(toml::Value::Float(f64::from(
                            1.0 - transparency.unwrap_or(0.0),
                        ))),
                        4 => {}
                        _ => bail!("floor.albedo must contain three legacy or four RGBA channels"),
                    }
                }
                None if transparency.is_some() => {
                    let mut channels = FloorConfig::default()
                        .albedo
                        .into_iter()
                        .map(|channel| toml::Value::Float(f64::from(channel)))
                        .collect::<Vec<_>>();
                    channels[3] = toml::Value::Float(f64::from(1.0 - transparency.unwrap()));
                    floor.insert("albedo".into(), toml::Value::Array(channels));
                }
                None => {}
            }
        }

        if let Some(window) = root.get_mut("window").and_then(toml::Value::as_table_mut) {
            let samples = window.remove("texture_samples");
            let temporal = window.remove("temporal_texture_aa");

            if let Some(value) = window.get_mut("texture_aa") {
                if let Some(mode) = value.as_str()
                    && let Some(canonical) = match mode {
                        "ss2x2" | "s_s2x2" => Some("super_sample2x2"),
                        "ss4" => Some("super_sample4"),
                        "ss4x2" | "four_by_two" => Some("super_sample4x2"),
                        "ss8" => Some("super_sample8"),
                        "ss8x2" => Some("super_sample8x2"),
                        "ss16" => Some("super_sample16"),
                        _ => None,
                    }
                {
                    *value = toml::Value::String(canonical.into());
                }
            } else if samples.is_some() || temporal.is_some() {
                let samples = samples
                    .map(|value| {
                        value
                            .as_integer()
                            .context("window.texture_samples must be an integer")
                    })
                    .transpose()?
                    .unwrap_or(4);
                let temporal = temporal
                    .map(|value| {
                        value
                            .as_bool()
                            .context("window.temporal_texture_aa must be a boolean")
                    })
                    .transpose()?
                    .unwrap_or(true);
                let mode = match (samples, temporal) {
                    (1, _) => "nearest",
                    (4, true) => "super_sample2x2",
                    (4, false) => "super_sample4",
                    (8, true) => "super_sample4x2",
                    (8, false) => "super_sample8",
                    (16, true) => "super_sample8x2",
                    (16, false) => "super_sample16",
                    (samples, _) => {
                        bail!("window.texture_samples must be 1, 4, 8, or 16; got {samples}")
                    }
                };
                window.insert("texture_aa".into(), toml::Value::String(mode.into()));
            }
        }
    }

    value
        .as_table_mut()
        .context("configuration root must be a TOML table")?
        .insert(CONFIG_VERSION_KEY.into(), toml::Value::Integer(version));
    Ok(())
}

fn value_as_f32(value: &toml::Value, name: &str) -> Result<f32> {
    match value {
        toml::Value::Float(value) => Ok(*value as f32),
        toml::Value::Integer(value) => Ok(*value as f32),
        _ => bail!("{name} must be a number"),
    }
}
