//! Loads source video timing, camera metadata and matching motion sidecars.

use gyroflow_core::stabilization_params::ReadoutDirection;
use gyroflow_core::lens_profile::LensProfile;
use gyroflow_core::{GyroflowCoreError, StabilizationManager, filesystem, telemetry_parser};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};
use telemetry_parser::util::VideoMetadata;

const SIDECAR_METADATA: &str = "video_sidecar_metadata";

pub fn get_video_metadata<T: Read + Seek>(
    stream: &mut T,
    filesize: u64,
    url: &str,
) -> Result<VideoMetadata, GyroflowCoreError> {
    let filename = filesystem::get_filename(url);
    let extension = filename.rsplit_once('.').map(|(_, ext)| ext);
    if extension
        .is_some_and(|ext| ext.eq_ignore_ascii_case("nev") || ext.eq_ignore_ascii_case("r3d"))
    {
        // Core's extension allowlist excludes NEV/R3D. The container parser reads
        // Nikon's dimensions and timing without decoding the RAW image stream.
        // It also rejects non-QuickTime RED R3D and malformed files; an extension
        // alone must never supply guessed dimensions or frame rates.
        stream.seek(SeekFrom::Start(0))?;
        Ok(telemetry_parser::util::get_video_metadata(
            stream, filesize,
        )?)
    } else {
        // Preserve core's format-specific handling, including BRAW clip timing.
        gyroflow_core::util::get_video_metadata(stream, filesize, url)
    }
}

pub fn load_video_file<T: Read + Seek>(
    stab: &StabilizationManager,
    stream: &mut T,
    filesize: u64,
    url: &str,
) -> Result<VideoMetadata, GyroflowCoreError> {
    let metadata = get_video_metadata(stream, filesize, url)?;
    stream.seek(SeekFrom::Start(0))?;
    let metadata = stab.load_video_file(stream, filesize, url, Some(metadata), true)?;
    if metadata.width > 0 && metadata.height > 0 && metadata.duration_s > 0.0 && metadata.fps > 0.0
    {
        autoload_local_zoom_profile(stab)?;
        let path = filesystem::url_to_path(url);
        let path = Path::new(&path);
        // Core already checks sidecars for some containers. Fill in when that
        // lookup is skipped or its first candidate has no usable motion.
        if stab.gyro.read().quaternions.len() < 2 {
            for extension in ["gcsv", "GCSV", "csv", "CSV"] {
                if load_motion_sidecar(stab, url, &path.with_extension(extension)) {
                    break;
                }
            }
        }
        for extension in ["json", "JSON"] {
            if load_metadata_sidecar(stab, &path.with_extension(extension)) {
                break;
            }
        }
    }
    Ok(metadata)
}

pub(crate) fn zoom_focal_range(profile: &LensProfile) -> Option<(f64, f64)> {
    let anchors = profile.interpolations.as_ref()?.as_object()?;
    let (min, max) = anchors.iter().filter_map(|(key, anchor)| {
        let mm = anchor.get("focal_length")?.as_f64()?;
        let key: f64 = key.parse().ok()?;
        (mm.is_finite() && mm > 0.0 && (key - mm).abs() <= 0.000001).then_some(mm)
    }).fold((f64::INFINITY, 0.0_f64), |(min, max), mm| (min.min(mm), max.max(mm)));
    (max > min).then_some((min, max))
}

fn autoload_local_zoom_profile(stab: &StabilizationManager) -> Result<(), GyroflowCoreError> {
    let Some(camera) = stab.camera_id.read().clone() else { return Ok(()); };
    if camera.identifier.is_empty() || camera.lens_model.trim().is_empty() {
        return Ok(());
    }
    let (min, max) = {
        let gyro = stab.gyro.read();
        let metadata = gyro.file_metadata.read();
        if metadata.lens_profile.as_ref().is_some_and(Value::is_object) {
            return Ok(());
        }
        metadata.lens_positions.values().filter(|f| f.is_finite() && **f > 0.0)
            .fold((f64::INFINITY, 0.0_f64), |(min, max), &f| (min.min(f), max.max(f)))
    };
    if !min.is_finite() {
        return Ok(());
    }
    let id = camera.get_identifier_for_autoload();
    let selection = {
        let db = stab.lens_profile_db.read();
        if max <= min * 1.002 && db.contains_id(&id) {
            return Ok(());
        }
        let local = gyroflow_core::settings::data_dir().join("lens_profiles");
        let lens_name = |name: &str| {
            let name = name.trim().to_ascii_lowercase();
            let name = if camera.brand.eq_ignore_ascii_case("Sony") {
                name.strip_prefix("e ").or_else(|| name.strip_prefix("tamron ")).unwrap_or(&name)
            } else { &name };
            name.split_whitespace().collect::<String>()
        };
        let camera_lens = lens_name(&camera.lens_model);
        db.get_all_filenames().into_iter().filter_map(|path| {
            if !Path::new(&path).starts_with(&local) {
                return None;
            }
            let profile = db.find(&path)?;
            if !profile.has_interpolations()
                || !profile.camera_brand.eq_ignore_ascii_case(&camera.brand)
                || !profile.camera_model.eq_ignore_ascii_case(&camera.model)
                || lens_name(&profile.lens_model) != camera_lens
                || (profile.calib_dimension.w, profile.calib_dimension.h) != (camera.video_width, camera.video_height)
                || (profile.fps * 1000.0).round() as usize != camera.fps
                || (!profile.camera_setting.is_empty() && !camera.camera_setting.is_empty()
                    && !profile.camera_setting.eq_ignore_ascii_case(&camera.camera_setting)) {
                return None;
            }
            let (start, end) = zoom_focal_range(profile)?;
            (min >= start - 0.0001 && max <= end + 0.0001).then(|| {
                (end - start, if profile.identifier.is_empty() { path } else { profile.identifier.clone() })
            })
        }).min_by(|(a, ka), (b, kb)| a.total_cmp(b).then(ka.cmp(kb))).map(|(_, key)| key)
    };
    if let Some(key) = selection {
        stab.load_lens_profile(&key)?;
    }
    Ok(())
}

fn load_motion_sidecar(stab: &StabilizationManager, video_url: &str, path: &Path) -> bool {
    let url = filesystem::path_to_url(&path.to_string_lossy());
    let Ok(mut file) = filesystem::open_file(&url, false, false) else {
        return false;
    };
    let size = file.size;
    // Core clears motion before parsing. Stage the load so an invalid sidecar
    // cannot replace the clip's existing metadata and timing.
    let candidate = stab.get_cloned();
    candidate.input_file.write().url = video_url.to_owned();
    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        candidate.load_gyro_data(
            file.get_file(),
            size,
            &url,
            false,
            &Default::default(),
            |_| (),
            Arc::new(AtomicBool::new(false)),
        )
    }));
    if !matches!(loaded, Ok(Ok(()))) || candidate.gyro.read().quaternions.len() < 2 {
        log::warn!("Ignoring unusable motion sidecar: {}", path.display());
        return false;
    }

    let mut gyro = std::mem::take(&mut *candidate.gyro.write());
    // Retain metadata from the video even though its motion comes from a different file.
    let source_metadata = stab
        .gyro
        .read()
        .file_metadata
        .read()
        .additional_data
        .clone();
    if let Some(source_metadata) = source_metadata.as_object().filter(|data| !data.is_empty()) {
        let mut metadata = gyro.file_metadata.read().clone();
        if let Some(data) = metadata.additional_data.as_object_mut() {
            for (key, value) in source_metadata {
                data.entry(key).or_insert_with(|| value.clone());
            }
        }
        gyro.file_metadata = metadata.into();
    }
    *stab.gyro.write() = gyro;
    stab.input_file.write().url = video_url.to_owned();
    log::info!("Loaded motion sidecar: {}", path.display());
    true
}

fn load_metadata_sidecar(stab: &StabilizationManager, path: &Path) -> bool {
    let Ok(data) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(metadata @ Value::Object(_)) = serde_json::from_str(data.trim_start_matches('\u{feff}'))
    else {
        log::warn!("Ignoring invalid JSON sidecar: {}", path.display());
        return false;
    };

    let mut gyro = stab.gyro.write();
    let mut file_metadata = gyro.file_metadata.read().clone();
    if !file_metadata.additional_data.is_object() {
        file_metadata.additional_data = serde_json::json!({});
    }
    file_metadata.additional_data[SIDECAR_METADATA] = metadata;
    gyro.file_metadata = file_metadata.into();
    log::info!("Loaded clip metadata sidecar: {}", path.display());
    true
}

pub(crate) fn sidecar_metadata(stab: &StabilizationManager) -> Value {
    stab.gyro.read().file_metadata.read().additional_data[SIDECAR_METADATA].clone()
}

pub(crate) fn sidecar_readout(stab: &StabilizationManager) -> Option<(f64, ReadoutDirection)> {
    let gyro = stab.gyro.read();
    let metadata = gyro.file_metadata.read();
    let sidecar = &metadata.additional_data[SIDECAR_METADATA];
    let time = sidecar["frame_readout_time"]
        .as_f64()
        .filter(|time| time.is_finite())?;
    let direction = serde_json::from_value(sidecar["frame_readout_direction"].clone()).unwrap_or(
        if time < 0.0 {
            ReadoutDirection::BottomToTop
        } else {
            ReadoutDirection::TopToBottom
        },
    );
    Some((time.abs(), direction))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn raw_sample_entries_preserve_timing_for_both_extensions() {
        // A synthetic black clip supplies real QuickTime track tables. Change
        // the sample-entry fourcc and discard AVC-specific configuration;
        // no compressed image is decoded here.
        for codec in [b"NRAW", b"NR3D"] {
            let mut bytes = include_bytes!("../tests/fixtures/video-timing.mp4").to_vec();
            let config = bytes.windows(4).position(|value| value == b"avcC").unwrap();
            bytes[config..config + 4].copy_from_slice(b"free");
            let positions: Vec<_> = bytes
                .windows(4)
                .enumerate()
                .filter_map(|(i, value)| (value == b"avc1").then_some(i))
                .collect();
            assert!(!positions.is_empty());
            for position in positions {
                bytes[position..position + 4].copy_from_slice(codec);
            }
            for extension in ["NEV", "nev", "R3D", "r3d"] {
                let url = filesystem::path_to_url(&format!("C:/clip.{extension}"));
                let mut stream = Cursor::new(&bytes);
                let metadata = get_video_metadata(&mut stream, bytes.len() as u64, &url).unwrap();
                assert_eq!((metadata.width, metadata.height), (16, 16));
                assert!((metadata.fps - 24000.0 / 1001.0).abs() < 1e-6);
                assert!((metadata.duration_s - 2.0 * 1001.0 / 24000.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn raw_extensions_do_not_accept_invalid_containers() {
        for extension in ["NEV", "nev", "R3D", "r3d"] {
            let mut stream = Cursor::new(vec![0u8; 64]);
            let error = get_video_metadata(
                &mut stream,
                64,
                &filesystem::path_to_url(&format!("C:/clip.{extension}")),
            )
            .unwrap_err();
            assert!(
                !matches!(error, GyroflowCoreError::UnsupportedFormat(_)),
                "RAW files must reach the container parser"
            );
        }
        let mut stream = Cursor::new(vec![0u8; 64]);
        assert!(matches!(
            get_video_metadata(&mut stream, 64, &filesystem::path_to_url("C:/clip.nev.bak")),
            Err(GyroflowCoreError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn video_load_uses_matching_motion_sidecars() {
        let directory =
            std::env::temp_dir().join(format!("gyroflow-sidecars-{}", fastrand::u64(..)));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("clip.with.dots.MOV");
        let video = include_bytes!("../tests/fixtures/video-timing.mp4");
        std::fs::write(&path, video).unwrap();
        std::fs::write(path.with_extension("gcsv"), "unrecognized format").unwrap();
        std::fs::write(
            path.with_extension("CSV"),
            "CAMERA IMU LOG\nversion,1.3\nid,Sigma_FP2\norientation,XYZ\ntscale,0.001\ngscale,1\nt,gx,gy,gz\n0,1,2,3\n25,2,3,4\n50,3,4,5\n75,4,5,6\n",
        ).unwrap();
        std::fs::write(
            path.with_extension("JSON"),
            br#"{"camera_brand":"Sigma","camera_model":"FP2","lens_model":"58mm F2.0","frame_readout_time":15.237}"#,
        ).unwrap();
        let stab = StabilizationManager::default();
        let url = filesystem::path_to_url(&path.to_string_lossy());
        let metadata =
            load_video_file(&stab, &mut Cursor::new(video), video.len() as u64, &url).unwrap();
        assert_eq!((metadata.width, metadata.height), (16, 16));
        assert_eq!(stab.params.read().frame_count, 2);
        assert_eq!(stab.input_file.read().url, url);
        assert_eq!(sidecar_metadata(&stab)["lens_model"], "58mm F2.0");
        assert_eq!(sidecar_metadata(&stab)["frame_readout_time"], 15.237);
        assert!(stab.gyro.read().quaternions.len() >= 2);
        assert!(
            filesystem::get_filename(&stab.gyro.read().file_url)
                .eq_ignore_ascii_case("clip.with.dots.csv")
        );
        assert!(
            !stab
                .gyro
                .read()
                .file_metadata
                .read()
                .has_accurate_timestamps
        );
        std::fs::write(
            path.with_extension("gcsv"),
            "GYROFLOW IMU LOG\nversion,1.3\nid,Test\norientation,XYZ\nt,gx,gy,gz\n0,1,2,3\n25,2,3,4\n50,3,4,5\n75,4,5,6\n",
        ).unwrap();
        let stab = StabilizationManager::default();
        load_video_file(&stab, &mut Cursor::new(video), video.len() as u64, &url).unwrap();
        assert_eq!(
            stab.gyro
                .read()
                .file_metadata
                .read()
                .detected_source
                .as_deref(),
            Some("gcsv Test")
        );

        std::fs::remove_file(path.with_extension("gcsv")).unwrap();
        std::fs::remove_file(path.with_extension("CSV")).unwrap();
        std::fs::remove_file(path.with_extension("JSON")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    /// Run with GYROFLOW_SIGMA_SAMPLE_DIR pointing at the original camera files.
    #[test]
    #[ignore = "requires the Sigma FP2 recording and sidecars"]
    fn sigma_fp2_sample_loads_motion() {
        let directory =
            std::env::var_os("GYROFLOW_SIGMA_SAMPLE_DIR").expect("set GYROFLOW_SIGMA_SAMPLE_DIR");
        let path = Path::new(&directory).join("A001_007_20261119.MOV");
        let url = filesystem::path_to_url(&path.to_string_lossy());
        let mut stream = std::fs::File::open(&path).unwrap();
        let size = stream.metadata().unwrap().len();
        let stab = StabilizationManager::default();
        let video = load_video_file(&stab, &mut stream, size, &url).unwrap();
        assert_eq!((video.width, video.height), (3424, 2576));
        let sample_count = stab.gyro.read().quaternions.len();
        assert!(sample_count > 1000);
        assert_eq!(sidecar_metadata(&stab)["camera_model"], "FP2");
        assert_eq!(sidecar_metadata(&stab)["lens_model"], "58mm F2.0");
        let (time, direction) = sidecar_readout(&stab).unwrap();
        assert_eq!(time, 15.237);
        assert!(matches!(direction, ReadoutDirection::TopToBottom));
        assert_eq!(
            stab.gyro
                .read()
                .file_metadata
                .read()
                .detected_source
                .as_deref(),
            Some("gcsv Sigma FP2")
        );
        println!(
            "{}x{} at {:.6} fps, {sample_count} integrated samples",
            video.width, video.height, video.fps
        );
    }

    /// Run with GYROFLOW_NIKON_SAMPLE_DIR pointing at the original camera files.
    #[test]
    #[ignore = "requires Nikon ZR camera samples"]
    fn nikon_zr_samples_load_with_original_paths() {
        let directory =
            std::env::var_os("GYROFLOW_NIKON_SAMPLE_DIR").expect("set GYROFLOW_NIKON_SAMPLE_DIR");
        for (name, frames) in [
            ("DSC_0073.NEV", 147),
            ("DSC_0073.R3D", 147),
            ("DSC_0003.R3D", 15),
        ] {
            let path = std::path::Path::new(&directory).join(name);
            let url = filesystem::path_to_url(&path.to_string_lossy());
            let mut stream = std::fs::File::open(&path).unwrap();
            let size = stream.metadata().unwrap().len();
            let stab = StabilizationManager::default();
            let metadata = load_video_file(&stab, &mut stream, size, &url).unwrap();
            assert_eq!((metadata.width, metadata.height), (6060, 3410));
            assert!((metadata.fps - 24000.0 / 1001.0).abs() < 1e-6);
            assert!((metadata.duration_s - frames as f64 * 1001.0 / 24000.0).abs() < 1e-6);
            assert_eq!(stab.params.read().frame_count, frames);

            stream.seek(SeekFrom::Start(0)).unwrap();
            let input = telemetry_parser::Input::from_stream(
                &mut stream,
                size,
                &path,
                |_| (),
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .unwrap();
            assert_eq!(input.camera_type(), "Nikon");
            let imu = telemetry_parser::util::normalized_imu(&input, None).unwrap();
            println!(
                "{name}: {metadata:?}, camera={:?}, metadata samples={}, IMU samples={}",
                input.camera_model(),
                input.samples.as_ref().map_or(0, Vec::len),
                imu.len()
            );
        }
    }
}
