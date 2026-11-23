//! The "Generic profile" settings: a lens nobody calibrated, sized from its focal length, in place of the clip's
//! lens profile. A SIGMA fp CinemaDNG take gets it per frame through `fsg2` (the camera reports its sensor mode); any
//! other clip gets a lens profile synthesized here.
use gyroflow_core::StabilizationManager;
use gyroflow_core::lens_profile::{ CameraParams, Dimensions, LensProfile };
use crate::{ fsg2, lensfit };

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManualLens {
    pub focal_mm: f64,
    /// The corner distortion against an ideal lens (a fraction), None = the typical one for the focal length
    pub corner: Option<f64>,
    /// The width of the sensor area the clip records, in mm, when the clip doesn't report it
    pub sensor_width_mm: f64,
}

impl ManualLens {
    pub fn fsg2_options(&self) -> fsg2::Options {
        fsg2::Options { manual_focal_mm: Some(self.focal_mm), manual_corner: self.corner }
    }

    /// An ideal rectilinear lens in Gyroflow's (OpenCV fisheye) model, with the barrel of `lensfit::generic_rectilinear`
    pub fn profile(&self, width: usize, height: usize, sensor_width_mm: f64) -> LensProfile {
        let (w, h) = (width as f64, height as f64);
        let fx = w * self.focal_mm / sensor_width_mm;
        let coeffs = lensfit::generic_rectilinear(self.focal_mm, width as u32, height as u32, fx, self.corner);
        let mut profile = LensProfile::default();
        profile.lens_model = format!("Generic profile {} mm", self.focal_mm);
        profile.note = format!("Uncalibrated: {:.2} mm sensor width, {}", sensor_width_mm,
            self.corner.map_or("typical distortion".to_owned(), |c| format!("corner distortion {:+.1} %", c * 100.0)));
        profile.calib_dimension = Dimensions { w: width, h: height };
        profile.orig_dimension = Dimensions { w: width, h: height };
        profile.input_horizontal_stretch = 1.0;
        profile.input_vertical_stretch = 1.0;
        profile.focal_length = Some(self.focal_mm);
        profile.fisheye_params = CameraParams {
            camera_matrix: vec![[fx, 0.0, w / 2.0], [0.0, fx, h / 2.0], [0.0, 0.0, 1.0]],
            distortion_coeffs: coeffs.to_vec(),
            ..Default::default()
        };
        profile.init();
        profile.finalize();
        profile
    }
}

/// The width of the sensor area the clip records, in mm, from its lens metadata: pixel pitch times capture area
/// (Sony), or the frame width at the focal length in pixels (SIGMA fp CinemaDNG)
pub fn clip_sensor_width_mm(stab: &StabilizationManager) -> Option<f64> {
    let width = stab.params.read().size.0 as f64;
    let gyro = stab.gyro.read();
    let md = gyro.file_metadata.read();
    let valid = |mm: f64| (mm.is_finite() && mm > 1.0 && mm < 100.0).then_some(mm);
    md.lens_params.values().find_map(|l| {
        let (pitch, area) = (l.pixel_pitch?, l.capture_area_size?);
        valid(pitch.0 as f64 / 1_000_000.0 * area.0 as f64)
    }).or_else(|| md.lens_params.values().find_map(|l| {
        let (mm, px) = (l.focal_length? as f64, l.pixel_focal_length?.0 as f64);
        valid(width * mm / px)
    }))
}

/// The focal length (mm) and the sensor width (mm) of the clip's lens metadata, where it has them
pub fn clip_lens_metadata(stab: &StabilizationManager) -> (Option<f64>, Option<f64>) {
    let focal_mm = {
        let gyro = stab.gyro.read();
        let md = gyro.file_metadata.read();
        md.lens_params.values().find_map(|l| l.focal_length.map(f64::from).filter(|mm| mm.is_finite() && *mm > 0.0))
    };
    (focal_mm, clip_sensor_width_mm(stab))
}

/// Replaces the lens with the generic one, sized on the clip's own sensor area when it reports it
pub fn apply(stab: &StabilizationManager, lens: &ManualLens) -> crate::PluginResult<()> {
    let (width, height) = stab.params.read().size;
    if width == 0 || height == 0 { return Err("The clip has no size".into()); }
    let sensor_width_mm = clip_sensor_width_mm(stab).unwrap_or(lens.sensor_width_mm);
    let profile = lens.profile(width, height, sensor_width_mm);
    log::info!("Generic profile {lens:?} on {width}x{height}: sensor width {sensor_width_mm:.2} mm, fx {:.1}, coefficients {:?}",
        profile.fisheye_params.camera_matrix[0][0], profile.fisheye_params.distortion_coeffs);
    stab.load_lens_profile(&profile.get_json_value()?.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gyroflow_core::filesystem;

    /// Image radius in pixels of a ray `theta` off the axis, in the OpenCV fisheye model
    fn radius(fx: f64, k: &[f64], theta: f64) -> f64 {
        let t2 = theta * theta;
        fx * theta * (1.0 + k[0] * t2 + k[1] * t2 * t2 + k[2] * t2.powi(3) + k[3] * t2.powi(4))
    }

    #[test]
    fn full_frame_profile_from_the_focal_length() {
        let lens = ManualLens { focal_mm: 80.0, corner: Some(0.0), sensor_width_mm: 36.0 };
        let profile = lens.profile(3840, 2160, 36.0);
        assert!((profile.fisheye_params.camera_matrix[0][0] - 3840.0 * 80.0 / 36.0).abs() < 1e-9);
        // No corner distortion: rectilinear, the corner where tan(theta) * fx puts it
        let fx = profile.fisheye_params.camera_matrix[0][0];
        let corner = (1920.0f64).hypot(1080.0);
        let theta = (corner / fx).atan();
        assert!((radius(fx, &profile.fisheye_params.distortion_coeffs, theta) - corner).abs() < 0.01);
    }

    /// GYROFLOW_MANUAL_LENS_SONY_DIR=<folder of Sony clips of one zoom lens>, GYROFLOW_MANUAL_LENS_PROFILE=<its zoom profile>:
    /// the manual lens at each clip's own focal length against the calibration
    #[test]
    #[ignore = "requires Sony clips and their calibrated zoom profile"]
    fn sony_manual_lens_against_the_calibrated_zoom_profile() {
        let dir = std::env::var("GYROFLOW_MANUAL_LENS_SONY_DIR").expect("set GYROFLOW_MANUAL_LENS_SONY_DIR");
        let calibration = std::env::var("GYROFLOW_MANUAL_LENS_PROFILE").expect("set GYROFLOW_MANUAL_LENS_PROFILE");
        let mut clips: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mp4"))).collect();
        clips.sort();
        assert!(!clips.is_empty());
        for clip in clips {
            let path = clip.to_string_lossy().into_owned();
            let url = filesystem::path_to_url(&path);
            let stab = StabilizationManager::default();
            let mut file = filesystem::open_file(&url, false, false).unwrap();
            let size = file.size;
            crate::video_metadata::load_video_file(&stab, file.get_file(), size, &url).unwrap();
            let focal_mm = stab.gyro.read().file_metadata.read().lens_params.values().find_map(|l| l.focal_length).unwrap() as f64;
            let sensor_mm = clip_sensor_width_mm(&stab).expect("Sony clips report their sensor area");

            for corner in [None, Some(0.0)] {
                apply(&stab, &ManualLens { focal_mm, corner, sensor_width_mm: 36.0 }).unwrap();
                let manual = stab.lens.read().clone();
                let (fx, k) = (manual.fisheye_params.camera_matrix[0][0], manual.fisheye_params.distortion_coeffs.clone());

                let reference = StabilizationManager::default();
                *reference.params.write() = stab.params.read().clone();
                reference.load_lens_profile(&calibration).unwrap();
                let calibrated = reference.lens.read().get_interpolated_lens_at(focal_mm);
                let (cfx, ck) = (calibrated.fisheye_params.camera_matrix[0][0], calibrated.fisheye_params.distortion_coeffs.clone());

                // Where the calibration's ray lands against the manual lens's, at image radii the calibration covers (its
                // high-order coefficients run away towards the corners of this one)
                let errors: Vec<String> = [500.0, 1000.0, 1500.0, 1920.0].iter().map(|&px: &f64| {
                    let (mut lo, mut hi) = (0.0, 1.5);
                    for _ in 0..60 { let mid = (lo + hi) / 2.0; if radius(cfx, &ck, mid) < px { lo = mid } else { hi = mid } }
                    format!("{px:.0} px {:+.1}", radius(fx, &k, lo) - px)
                }).collect();
                println!("{} {focal_mm:>5.1} mm, sensor {sensor_mm:.2} mm, {}: fx {fx:.0} vs calibrated {cfx:.0} ({:+.2} %), radius error {}",
                    clip.file_name().unwrap().to_string_lossy(), if corner.is_none() { "typical distortion" } else { "no distortion     " },
                    (fx / cfx - 1.0) * 100.0, errors.join(", "));
                assert!((fx / cfx - 1.0).abs() < 0.05, "focal within 5 % of the calibration");
            }
        }
    }
}
