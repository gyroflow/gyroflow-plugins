//! The SIGMA fp's own lens distortion, turned into Gyroflow's numbers.
//!
//! A port of fpSup's on-camera fit (gyro/distfit.inc.S and gcsv_dist.S), kept
//! to the same arithmetic so its output can be checked against Gyro1's and
//! against fpSup's Python mirror of the assembly (gyro/test_distfit.py).
//!
//! The firmware keeps, for the mounted lens, five focus support points, each
//! with four radial coefficients per colour plane: the numbers it writes into
//! a DNG's WarpRectilinear opcode, which maps a corrected radius rho (of the
//! corner distance) to a source radius rho * (kr0 + kr1 rho^2 + kr2 rho^4 +
//! kr3 rho^6). Interpolating the table linearly at the focus distance
//! reproduces a real DNG's opcode to eight places.
//!
//! kr0 is not distortion: it is focus breathing, a magnification. It goes on
//! the focal length and is divided out before the shape is fitted to
//! Gyroflow's OpenCV fisheye model, theta + k1 theta^3 + k2 theta^5 + k3 theta^7,
//! by collocation at three radii.

/// The plane luminance follows.
const PLANE_GREEN: usize = 1;
/// Collocation radii, as fractions of the corner distance.
const RHO: [f64; 3] = [0.55, 0.85, 1.0];
const SENSOR_ACTIVE_W: f64 = 6000.0;
const SENSOR_ACTIVE_MM: f64 = 35.9;

/// The focal length to build the camera matrix from, in mm.
///
/// The correction data's own focal length wins over the one the mount reports
/// (a SIGMA 40mm F1.4 Art reports 40.0 and was calibrated at 39.4), unless it
/// is implausible: under 1 mm, over 2 m, or more than a quarter away from the
/// mount's number. Same rule as pg_dist_focal.
pub fn focal_mm(calib_tenths_mm: Option<u32>, mount_mm: Option<f64>) -> Option<f64> {
    let mount_tenths = mount_mm.filter(|mm| mm.is_finite() && *mm > 0.0).map(|mm| (mm * 10.0).round() as i64);
    match (calib_tenths_mm.map(i64::from), mount_tenths) {
        (Some(calib), Some(mount)) if (10..=20000).contains(&calib) && mount >= 4 * (calib - mount).abs() => Some(calib as f64 / 10.0),
        (Some(calib), None) if (10..=20000).contains(&calib) => Some(calib as f64 / 10.0),
        (_, Some(mount)) => Some(mount as f64 / 10.0),
        _ => None,
    }
}

/// Focal length in output pixels for a mode that images `covered_cols` sensor columns.
pub fn focal_px(frame_w: u32, focal_mm: f64, covered_cols: u16) -> f64 {
    let covered = if covered_cols == 0 { SENSOR_ACTIVE_W } else { (covered_cols as f64).min(SENSOR_ACTIVE_W) };
    frame_w as f64 * focal_mm / (SENSOR_ACTIVE_MM * covered / SENSOR_ACTIVE_W)
}

/// Coefficients for a lens nobody calibrated: Gyroflow's OpenCV fisheye model
/// of an ideal rectilinear lens (the series of atan: tan(t) = t + t^3/3 +
/// 2t^5/15 + 17t^7/315 + 62t^9/2835), with k1 moved so the frame corner gets
/// the barrel typical of the focal length. The typical barrel is the median of
/// 111 SIGMA fp profiles in Gyroflow's lens database (2026-10-07), at the
/// corner against the ideal lens: under 15 mm -1.8 %, 15-24 mm -3.0 %,
/// 24-35 mm -3.3 %, 35-60 mm -1.1 %, from 60 mm -0.2 % (spread under 24 mm
/// is wide: a calibrated profile does better there).
/// `corner` overrides the typical barrel: the corner's distortion against the
/// ideal lens as a fraction (-0.05 = 5 % barrel, 0 = the ideal lens).
pub fn generic_rectilinear(focal_mm: f64, width: u32, height: u32, fx: f64, corner: Option<f64>) -> [f64; 4] {
    let mut k = [1.0 / 3.0, 2.0 / 15.0, 17.0 / 315.0, 62.0 / 2835.0];
    let barrel = corner.unwrap_or(match focal_mm {
        f if f < 15.0 => -0.018,
        f if f < 24.0 => -0.030,
        f if f < 35.0 => -0.033,
        f if f < 60.0 => -0.011,
        _ => -0.002,
    });
    let corner = (width as f64).hypot(height as f64) / 2.0 / fx;  // tan of the corner angle
    let t = corner.atan();
    let ideal = t * (1.0 + k[0] * t.powi(2) + k[1] * t.powi(4) + k[2] * t.powi(6) + k[3] * t.powi(8));
    k[0] += barrel * ideal / t.powi(3);
    k
}

/// The green plane's four coefficients at a focus distance, as pg_dist_kr
/// computes them. `distance_mm` of `None`, zero, negative or infinite means
/// infinity. Returns `None` for a lens without correction data.
pub fn interpolate(axis: &[u32; 5], nodes: &[[[f64; 4]; 3]; 5], distance_mm: Option<f64>) -> Option<[f64; 4]> {
    let a = match distance_mm {
        Some(mm) if mm.is_finite() && mm > 0.0 => 16_777_216.0 / mm,
        _ => 0.0,
    };
    let mut i = 0;
    for j in 1..4 {
        if a >= axis[j] as f64 {
            i = j;
        }
    }
    let span = axis[i + 1] as f64 - axis[i] as f64;
    if span == 0.0 {
        return None;
    }
    let t = ((a - axis[i] as f64) / span).clamp(0.0, 1.0);
    let (lo, hi) = (nodes[i][PLANE_GREEN], nodes[i + 1][PLANE_GREEN]);
    let kr = std::array::from_fn(|k| lo[k] + t * (hi[k] - lo[k]));
    valid(kr).then_some(kr)
}

/// A table left zeroed by a lens with no data shows as kr0 far from one.
pub fn valid(kr: [f64; 4]) -> bool {
    kr.iter().all(|v| v.is_finite()) && kr[0] > 0.5 && kr[0] < 1.5
}

/// Fit Gyroflow's fisheye coefficients to one set of DNG radial coefficients.
///
/// `frame_w`/`frame_h` are the developed picture, `focal_px` the matrix focal
/// before breathing. Returns the four coefficients (the fourth is always zero)
/// and the focal length with the breathing folded in, for both diagonal
/// entries of the camera matrix.
pub fn fit(kr: [f64; 4], frame_w: u32, frame_h: u32, focal_px: f64) -> Option<([f64; 4], f64)> {
    if !valid(kr) || !(focal_px.is_finite() && focal_px > 0.0) {
        return None;
    }
    // The corner radius in focal lengths, which the fit is done against.
    let s = (frame_w as f64).hypot(frame_h as f64) / 2.0 / focal_px;
    let mut m = [[0.0; 3]; 3];
    let mut b = [0.0; 3];
    for (row, rho) in RHO.iter().enumerate() {
        let u = rho * s;
        let r2 = rho * rho;
        let g = (kr[0] + kr[1] * r2 + kr[2] * r2 * r2 + kr[3] * r2 * r2 * r2) / kr[0];
        let theta = u.atan();
        let t2 = theta * theta;
        m[row] = [theta * t2, theta * t2 * t2, theta * t2 * t2 * t2];
        b[row] = g * u - theta;
    }
    let coefficients = solve3(m, b)?;
    Some(([coefficients[0], coefficients[1], coefficients[2], 0.0], focal_px * kr[0]))
}

/// Cramer's rule, as pg_solve3.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: &[[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(&m);
    if d == 0.0 || !d.is_finite() {
        return None;
    }
    let mut out = [0.0; 3];
    for (column, value) in out.iter_mut().enumerate() {
        let mut n = m;
        for row in 0..3 {
            n[row][column] = b[row];
        }
        *value = det(&n) / d;
    }
    Some(out)
}

/// The green plane of the first WarpRectilinear in a DNG opcode list
/// (big-endian, as the DNG spec defines opcode lists).
pub fn warp_rectilinear(opcode_list: &[u8]) -> Option<[f64; 4]> {
    let be_u32 = |at: usize| opcode_list.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let be_f64 = |at: usize| opcode_list.get(at..at + 8).map(|b| f64::from_be_bytes(b.try_into().unwrap()));
    let count = be_u32(0)?;
    let mut at = 4;
    for _ in 0..count.min(64) {
        let id = be_u32(at)?;
        let size = be_u32(at + 12)? as usize;
        let body = at + 16;
        if id == 1 {
            let planes = be_u32(body)? as usize;
            if planes == 0 || planes > 4 {
                return None;
            }
            let plane = if planes >= 3 { PLANE_GREEN } else { 0 };
            let base = body + 4 + plane * 6 * 8;
            let kr = [be_f64(base)?, be_f64(base + 8)?, be_f64(base + 16)?, be_f64(base + 24)?];
            return valid(kr).then_some(kr);
        }
        at = body.checked_add(size)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // The LUMIX S 40/F2, read off the camera (fpSup gyro/test_distfit.py).
    const AXIS: [u32; 5] = [0, 18641, 37283, 49637, 55924];
    const GREEN: [[f64; 4]; 5] = [
        [0.987878098, 0.000230792, -0.001804660, 0.012710940],
        [0.995612771, -0.003803673, -0.003186222, 0.011056404],
        [0.999854447, -0.010596089, 0.003693395, 0.003874420],
        [0.999856513, -0.016680599, 0.013089761, -0.003420052],
        [0.999845119, -0.020184787, 0.018855619, -0.007281209],
    ];

    fn nodes() -> [[[f64; 4]; 3]; 5] {
        GREEN.map(|kr| [kr, kr, kr])
    }

    /// Golden values from fpSup's Python mirror of the assembly
    /// (asm_interpolate / asm_fit) for 1936x1090 at 40.0 mm:
    /// distance, kr, coefficients k1..k3, focal with breathing.
    const GOLDEN: [(f64, [f64; 4], [f64; 3], f64); 6] = [
        (f64::INFINITY, [9.878780980000000e-01, 2.307920000000000e-04, -1.804660000000000e-03, 1.271094000000000e-02], [3.434034356654717e-01, -9.580878466073323e-02, 1.931069370926159e+00], 2130.954872120334),
        (900.0, [9.956128508894739e-01, -3.803800931162369e-03, -3.186092426458057e-03, 1.105626873155336e-02], [3.266573683372537e-01, -9.813155189668302e-02, 1.595721472338275e+00], 2147.639531278018),
        (450.0, [9.998543792456360e-01, -1.059598050141926e-02, 3.693285108515062e-03, 3.874534721340819e-03], [2.966047906818278e-01, 7.396930653660273e-02, 7.564556456382933e-01], 2156.788945091422),
        (364.0, [9.998559200330400e-01, -1.493427112102856e-02, 1.039289043740427e-02, -1.326450409078700e-03], [2.772084741693018e-01, 2.200148386305785e-01, 2.240453137977921e-01], 2156.792268728652),
        (300.0, [9.998451189999999e-01, -2.018478700000000e-02, 1.885561900000000e-02, -7.281209000000000e-03], [2.540190304919543e-01, 3.954371794339360e-01, -3.550155534415396e-01], 2156.768969787187),
        // Nearer than the last support point clamps to it.
        (250.0, [9.998451189999999e-01, -2.018478700000000e-02, 1.885561900000000e-02, -7.281209000000000e-03], [2.540190304919543e-01, 3.954371794339360e-01, -3.550155534415396e-01], 2156.768969787187),
    ];

    #[test]
    fn the_generic_lens_is_rectilinear_with_the_typical_corner_barrel() {
        let theta_d = |t: f64, k: &[f64; 4]| t * (1.0 + k[0] * t * t + k[1] * t.powi(4) + k[2] * t.powi(6) + k[3] * t.powi(8));
        let fx = 1600.0;
        let k = generic_rectilinear(50.0, 1920, 1080, fx, None);
        let t = (1920.0f64.hypot(1080.0) / 2.0 / fx).atan();
        // the corner: 1.1 % barrel against the ideal series (itself tan to 0.1 %)
        let ideal = [1.0 / 3.0, 2.0 / 15.0, 17.0 / 315.0, 62.0 / 2835.0];
        assert!((theta_d(t, &k) / theta_d(t, &ideal) - (1.0 - 0.011)).abs() < 1e-9);
        assert!((theta_d(t, &ideal) / t.tan() - 1.0).abs() < 1e-3);
        // a set corner distortion replaces the typical one
        let k = generic_rectilinear(50.0, 1920, 1080, fx, Some(-0.08));
        assert!((theta_d(t, &k) / theta_d(t, &ideal) - (1.0 - 0.08)).abs() < 1e-9);
        assert_eq!(generic_rectilinear(50.0, 1920, 1080, fx, Some(0.0)), ideal);
        // near the centre it is the ideal lens: theta_d = tan(theta)
        let s = 0.05;
        assert!((theta_d(s, &k) / s.tan() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn interpolation_and_fit_match_the_camera_arithmetic() {
        let focal = focal_px(1936, 40.0, 6000);
        assert!((focal - 2157.1030640668523).abs() < 1e-9);
        for (distance, kr_want, k_want, fx_want) in GOLDEN {
            let kr = interpolate(&AXIS, &nodes(), Some(distance)).unwrap();
            for (got, want) in kr.iter().zip(kr_want) {
                assert!((got - want).abs() < 1e-12, "{distance} mm: kr {got} vs {want}");
            }
            let (k, fx) = fit(kr, 1936, 1090, focal).unwrap();
            for (got, want) in k.iter().zip(k_want) {
                // libm atan against the assembly's series: well under 1e-9.
                assert!((got - want).abs() < 1e-8, "{distance} mm: k {got} vs {want}");
            }
            assert_eq!(k[3], 0.0);
            assert!((fx - fx_want).abs() < 1e-8, "{distance} mm: fx {fx} vs {fx_want}");
        }
    }

    #[test]
    fn exif_focus_distance_reproduces_a_real_dng_opcode() {
        // A LUMIX S 40/F2 clip (A001_150, 2026-09-28) records SubjectDistance
        // 0.3 m, and its WarpRectilinear green plane is exactly what the table
        // read off the camera gives at 300 mm: the EXIF distance is the focus
        // the camera interpolates at.
        let dng_opcode = [0.999845119443748, -0.0201847874638537, 0.0188556188550667, -0.00728120901028809];
        let kr = interpolate(&AXIS, &nodes(), Some(300.0)).unwrap();
        for (got, want) in kr.iter().zip(dng_opcode) {
            assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        }
    }

    #[test]
    fn unknown_focus_is_infinity() {
        assert_eq!(interpolate(&AXIS, &nodes(), None), interpolate(&AXIS, &nodes(), Some(f64::INFINITY)));
        assert_eq!(interpolate(&AXIS, &nodes(), Some(0.0)), interpolate(&AXIS, &nodes(), None));
    }

    #[test]
    fn fit_holds_the_shape_within_a_tenth_of_a_pixel() {
        let focal = focal_px(1936, 40.0, 6000);
        let s = 1936_f64.hypot(1090.0) / 2.0 / focal;
        for kr in GREEN {
            let (k, _) = fit(kr, 1936, 1090, focal).unwrap();
            let worst = (1..=16).map(|i| {
                let rho = i as f64 / 16.0;
                let r2 = rho * rho;
                let g = (kr[0] + kr[1] * r2 + kr[2] * r2 * r2 + kr[3] * r2 * r2 * r2) / kr[0];
                let th = (rho * s).atan();
                let model = th + k[0] * th.powi(3) + k[1] * th.powi(5) + k[2] * th.powi(7);
                (model - g * rho * s).abs() * focal
            }).fold(0.0, f64::max);
            assert!(worst < 0.1, "residual {worst} px");
        }
    }

    #[test]
    fn a_lens_without_data_gives_nothing() {
        assert_eq!(interpolate(&AXIS, &[[[0.0; 4]; 3]; 5], None), None);
        assert_eq!(fit([0.0; 4], 1936, 1090, 2000.0), None);
    }

    #[test]
    fn calibrated_focal_wins_unless_implausible() {
        assert_eq!(focal_mm(Some(394), Some(40.0)), Some(39.4));
        assert_eq!(focal_mm(Some(0), Some(40.0)), Some(40.0));
        assert_eq!(focal_mm(Some(900), Some(40.0)), Some(40.0));
        assert_eq!(focal_mm(None, Some(28.0)), Some(28.0));
        assert_eq!(focal_mm(Some(280), None), Some(28.0));
        assert_eq!(focal_mm(None, None), None);
    }

    #[test]
    fn crop_modes_scale_the_focal_length() {
        // A mode imaging half the sensor's width doubles the focal in pixels.
        let full = focal_px(1920, 28.0, 6000);
        assert!((focal_px(1920, 28.0, 3000) - 2.0 * full).abs() < 1e-9);
        assert_eq!(focal_px(1920, 28.0, 0), full);
        assert_eq!(focal_px(1920, 28.0, 6064), full);
    }

    #[test]
    fn warp_rectilinear_reads_the_green_plane() {
        let mut list = 1_u32.to_be_bytes().to_vec();
        let mut body = 3_u32.to_be_bytes().to_vec();
        for plane in 0..3 {
            for v in [1.0 + plane as f64 * 1e-4, -0.0149, 0.0101, -0.0012, 0.0, 0.0] {
                body.extend_from_slice(&f64::to_be_bytes(v));
            }
        }
        body.extend_from_slice(&0.5_f64.to_be_bytes());
        body.extend_from_slice(&0.5_f64.to_be_bytes());
        for v in [1_u32, 0x0103_0000, 0, body.len() as u32] { list.extend_from_slice(&v.to_be_bytes()); }
        list.extend_from_slice(&body);
        assert_eq!(warp_rectilinear(&list), Some([1.0001, -0.0149, 0.0101, -0.0012]));
        assert_eq!(warp_rectilinear(&list[..20]), None);
    }
}
