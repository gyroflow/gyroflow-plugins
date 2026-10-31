//! Metadata loading for host-decoded Nikon RAW footage. NRAW and R3D NE use
//! QuickTime containers, even when an NRAW file has been renamed to `.R3D`.
//! The host retains responsibility for RAW decoding and color controls.

use gyroflow_core::{GyroflowCoreError, StabilizationManager, filesystem, telemetry_parser};
use std::io::{Read, Seek, SeekFrom};
use telemetry_parser::util::VideoMetadata;

pub fn get_video_metadata<T: Read + Seek>(
    stream: &mut T,
    filesize: usize,
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
    filesize: usize,
    url: &str,
) -> Result<VideoMetadata, GyroflowCoreError> {
    let metadata = get_video_metadata(stream, filesize, url)?;
    stream.seek(SeekFrom::Start(0))?;
    // Keep the real URL for camera detection, sidecars, presets and persistence.
    stab.load_video_file(stream, filesize, url, Some(metadata), true)
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
                let metadata = get_video_metadata(&mut stream, bytes.len(), &url).unwrap();
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
            let size = stream.metadata().unwrap().len() as usize;
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
