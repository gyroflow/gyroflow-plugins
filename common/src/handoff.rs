//! Hand a parsed CinemaDNG take to the stock app as a self-contained project.
use std::path::{Path, PathBuf};
use std::io::Write;
use ciborium::value::Value;
use crate::{cdng, StabilizationManager, PluginResult, filesystem, gyroflow_core};

/// Keep integer timestamp keys intact while adapting the newer (fx, fy) field
/// to Gyroflow 1.6.3's scalar. Newer cores accept that scalar too. SIGMA fp's
/// fitted lenses have equal fx and fy; refuse an anisotropic projection rather
/// than silently lose its vertical focal length.
fn compatible_metadata(value: &mut Value) -> PluginResult<()> {
    let Value::Map(fields) = value else { return Err("Invalid project metadata".into()) };
    for (key, val) in fields {
        if key.as_text() != Some("lens_params") { continue; }
        let Value::Map(lenses) = val else { return Err("Invalid per-frame lens metadata".into()) };
        for (_, lens) in lenses {
            let Value::Map(fields) = lens else { return Err("Invalid lens metadata".into()) };
            for (key, focal) in fields {
                if key.as_text() != Some("pixel_focal_length") { continue; }
                if let Value::Array(pair) = focal {
                    if pair.len() != 2 || pair[0] != pair[1] {
                        return Err("This lens has unequal horizontal and vertical focal lengths; the stock 1.6.3 project format cannot represent them".into());
                    }
                    *focal = pair[0].clone();
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn project_data(stab: &StabilizationManager, source: &Path) -> PluginResult<String> {
    let paths = cdng::sequence_paths(source)?;
    let first = paths.first().ok_or("Empty DNG sequence")?;
    let stem = first.file_stem().and_then(|s| s.to_str()).ok_or("Invalid DNG name")?;
    let prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = &stem[prefix.len()..];
    let start: i32 = digits.parse()?;
    let extension = first.extension().and_then(|s| s.to_str()).ok_or("Invalid DNG extension")?;
    let pattern = first.with_file_name(format!("{prefix}%0{}d.{extension}", digits.len()));
    let data = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None)?;
    let mut project: serde_json::Value = serde_json::from_str(&data)?;
    let encoded = project["gyro_source"]["file_metadata"].as_str().ok_or("Project has no embedded telemetry")?;
    let mut metadata: Value = gyroflow_core::util::decompress_from_base91_cbor(encoded)?;
    compatible_metadata(&mut metadata)?;
    project["gyro_source"]["file_metadata"] = gyroflow_core::util::compress_to_base91_cbor(&metadata)
        .ok_or("Unable to compress project metadata")?.into();
    project["videofile"] = filesystem::path_to_url(&pattern.to_string_lossy()).into();
    // The app's UI probes a separate gyro source before importing the project.
    // Match the sequence URL so it uses the embedded metadata instead of
    // trying to parse the private DNG carrier as a separate telemetry file.
    project["gyro_source"]["filepath"] = project["videofile"].clone();
    project["image_sequence_start"] = start.into();
    project["image_sequence_fps"] = stab.params.read().fps.into();
    // The core made a bookmark for the single source frame, before we changed
    // videofile to a sequence pattern. Let the app use that absolute pattern.
    project.as_object_mut().unwrap().remove("videofile_bookmark");
    project["gyro_source"].as_object_mut().unwrap().remove("filepath_bookmark");
    if !StabilizationManager::project_has_motion_data(serde_json::to_string(&project)?.as_bytes()) {
        return Err("Project has no usable embedded motion data".into());
    }
    Ok(serde_json::to_string_pretty(&project)?)
}

pub(crate) fn save_project(stab: &StabilizationManager, source: &Path) -> PluginResult<PathBuf> {
    save_project_in(stab, source, &gyroflow_core::settings::data_dir().join("cinemadng-handoffs"))
}

fn save_project_in(stab: &StabilizationManager, source: &Path, root: &Path) -> PluginResult<PathBuf> {
    let data = project_data(stab, source)?;
    // The editor will keep referring to this project after the app closes or
    // the computer restarts, so it must not live in the system temporary folder.
    std::fs::create_dir_all(root)?;
    let folder = root.join(format!("cinemadng-gyroflow-{:032x}", fastrand::u128(..)));
    std::fs::create_dir(&folder)?;
    let name = source.file_stem().and_then(|s| s.to_str()).ok_or("Invalid DNG name")?;
    let path = folder.join(format!("{name}.gyroflow"));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    file.write_all(data.as_bytes())?;
    log::info!("CinemaDNG handoff project: {} ({} bytes)", path.display(), data.len());
    Ok(path)
}

