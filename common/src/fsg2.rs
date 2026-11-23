//! Decoder for FSG2 payload kind 2: the SIGMA fp's per-frame gyro record.
//!
//! The camera copies raw sensor counts into each DNG's MakerNote tail and does
//! no arithmetic; everything that turns counts into Gyroflow Protobuf units and
//! timestamps happens here. A sample's time is its global index (counted from
//! the start of the take) times the sample period; events and the frame mark
//! are positions in that same sample stream. Frames are timed from the
//! sensor's Vd events (see `vd_frame_positions`), not from the frame mark.
//!
//! Payload layout, all integers little-endian. Version 2 (the camera since
//! 2026-10-15) differs from version 1 only at 24..38 and at the end:
//!
//! ```text
//!   0 u8   fmt_version       1 or 2
//!   1 u8   orientation       0 = "xyz"
//!   2 u16  flags             see FLAG_*
//!   4 u32  clip_id           TickTimer at record start; equal across a take
//!   8 u32  frame_seq         native frame number, 0-based
//!  12 u32  sample_period_ps  gyro sample period in picoseconds
//!  16 f32  gscale            rad/s per gyro count
//!  20 f32  ascale            g per accelerometer count
//!     version 1:
//!  24 u32  readout_ns        rolling-shutter readout of the running mode
//!  28 u16  mode_id           sensor mode, for diagnostics only
//!  30 u16  crop_x, crop_y, crop_w, crop_h   readout window, sensor pixels
//!     version 2: the sensor mode as the camera's accessors give it
//!  24 u16  mode_id           sensor mode, 0 = none running
//!  26 u16  hmax              sensor clocks (72 MHz) per line
//!  28 u16  read_w            columns read, before horizontal binning
//!  30 u16  lines             lines read, before vertical binning
//!  32 u16  bin_h, bin_v      binning (0 is read as 1)
//!  36 u16  calib_focal       focal length the lens's correction data was
//!                            measured at, tenths of a mm; 0 = none
//!     both:
//!  38 u16  frame_mark        sample position of the frame hook, from base_idx
//!  40 u32  base_idx          global index of the first sample in this block
//!  44 u16  n_samples
//!  46 u16  n_events
//!  48      samples           n_samples x i16 x, y, z (raw counts)
//!          events            n_events x { u16 pos, u8 type, u8 len, value }
//!                              type 1 level: 3 x i16 raw accelerometer counts
//!                              type 3 Vd: u32 TickTimer us (diagnostics only)
//!                              (type 2 focus is reserved; the camera does not write it)
//!          lens table        version 1 only, with FLAG_LENS_TABLE, LENS_TABLE_BYTES
//!
//! Version 1 blocks end at their hook, so frame_mark equals n_samples. A
//! version 2 block may end before it: when more samples are due than its tag
//! holds (a take's first frame, a stall), it carries the oldest and the next
//! blocks catch up. Blocks still tile the stream. The camera sets
//! FLAG_DATA_LOST and FLAG_FIRST_FRAME, and in version 1 also FLAG_TRUNCATED
//! and FLAG_LENS_TABLE, never the others. Version 2 has no lens table: each
//! DNG's own WarpRectilinear opcode is the distortion for its focus.
//! ```

use gyroflow_core::telemetry_parser::gyroflow::gyroflow_proto;

use crate::lensfit;

pub const FMT_VERSION: u8 = 1;
pub const FMT_VERSION_2: u8 = 2;
pub const HEADER_BYTES: usize = 48;

pub const FLAG_DATA_LOST: u16 = 1 << 0;
pub const FLAG_TRUNCATED: u16 = 1 << 1;
// Informational: the take's first and last blocks. The reader relies on
// clip_id and file order instead, so these are not checked.
#[allow(dead_code)]
pub const FLAG_FIRST_FRAME: u16 = 1 << 2;
#[allow(dead_code)]
pub const FLAG_LAST_FRAME: u16 = 1 << 3;
pub const FLAG_OIS_ON: u16 = 1 << 4;
pub const FLAG_EIS_ON: u16 = 1 << 5;
pub const FLAG_LENS_TABLE: u16 = 1 << 6;

pub const EVENT_LEVEL: u8 = 1;
pub const EVENT_FOCUS: u8 = 2;
/// The sensor's frame sync, Vd (IRQ 0x79): placed at the gyro stream position
/// where it fired, with the TickTimer (u32 microseconds) as its value.
pub const EVENT_VD: u8 = 3;

/// calib_focal u32 + five u32 focus support points + 5 x 3 x 4 f64 coefficients.
#[cfg_attr(not(test), allow(dead_code))]
pub const LENS_TABLE_BYTES: usize = 4 + 5 * 4 + 5 * 3 * 4 * 8;

/// IMX410 active area and pitch: 35.9 mm across 6000 pixels.
const SENSOR_PIXEL_WIDTH: u32 = 6000;
const SENSOR_PIXEL_HEIGHT: u32 = 4000;
const PIXEL_PITCH_NM: u32 = 5983;
const STANDARD_GRAVITY: f64 = 9.80665;

/// From a frame's Vd to the readout of its first sensor row, in microseconds.
/// Vd is fixed in the readout, not in the exposure: between 1/250 and 1/50 s
/// the frames' optical-flow centre moved by about minus half the exposure
/// (A001_012 and A001_016, 2026-10-07). Gyroflow's own autosync on both takes
/// then found the frames 0.9 ms late at 1.8 ms (medians of 8 sync points:
/// +1.25 and +0.93 ms; a one-frame shift of the images read +34.5): 0.9 ms,
/// to about +-1 ms.
pub const VD_TO_READOUT_US: f64 = 900.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub pos: u16,
    pub kind: u8,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LensTable {
    /// Focal length the correction data was measured at, in tenths of a mm.
    pub calib_focal_tenths_mm: u32,
    /// Focus support points, 2^24 / object distance in mm (0 = infinity).
    pub axis: [u32; 5],
    /// Five focus nodes x three colour planes x four radial coefficients.
    pub nodes: [[[f64; 4]; 3]; 5],
}

/// A version 2 block's sensor mode, as the camera read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorMode {
    pub hmax: u16,
    pub read_w: u16,
    pub lines: u16,
    pub bin_h: u16,
    pub bin_v: u16,
}

/// The IMX410's readout clock: hmax counts it.
const SENSOR_CLOCK_HZ: f64 = 72.0e6;

impl SensorMode {
    /// Readout of the frame, in ns (Gyro1's arithmetic).
    pub fn readout_ns(&self) -> u32 {
        (self.hmax as f64 * self.lines as f64 * 1.0e9 / SENSOR_CLOCK_HZ).round() as u32
    }

    /// The sensor area read, centred: x, y, w, h in sensor pixels.
    pub fn crop(&self) -> [u16; 4] {
        let w = (self.read_w as u32 * self.bin_h.max(1) as u32).min(SENSOR_PIXEL_WIDTH);
        let h = (self.lines as u32 * self.bin_v.max(1) as u32).min(SENSOR_PIXEL_HEIGHT);
        [((SENSOR_PIXEL_WIDTH - w) / 2) as u16, ((SENSOR_PIXEL_HEIGHT - h) / 2) as u16, w as u16, h as u16]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub fmt_version: u8,
    pub orientation: u8,
    pub flags: u16,
    pub clip_id: u32,
    pub frame_seq: u32,
    pub sample_period_ps: u32,
    pub gscale: f32,
    pub ascale: f32,
    /// Version 1 carries these; version 2 derives them from `sensor_mode`.
    pub readout_ns: u32,
    pub mode_id: u16,
    pub crop: [u16; 4],
    /// Version 2 only.
    pub sensor_mode: Option<SensorMode>,
    /// Version 2 only: the frame's calibrated focal length, tenths of a mm.
    pub calib_focal_tenths_mm: Option<u32>,
    pub frame_mark: u16,
    pub base_idx: u32,
    pub samples: Vec<[i16; 3]>,
    pub events: Vec<Event>,
    pub lens_table: Option<LensTable>,
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8], String> {
        let bytes = self.at.checked_add(len)
            .and_then(|end| self.data.get(self.at..end))
            .ok_or_else(|| format!("kind 2 payload truncated in {what}"))?;
        self.at += len;
        Ok(bytes)
    }
    fn u8(&mut self, what: &str) -> Result<u8, String> { Ok(self.take(1, what)?[0]) }
    fn u16(&mut self, what: &str) -> Result<u16, String> { Ok(u16::from_le_bytes(self.take(2, what)?.try_into().unwrap())) }
    fn i16(&mut self, what: &str) -> Result<i16, String> { Ok(i16::from_le_bytes(self.take(2, what)?.try_into().unwrap())) }
    fn u32(&mut self, what: &str) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take(4, what)?.try_into().unwrap())) }
    fn f32(&mut self, what: &str) -> Result<f32, String> { Ok(f32::from_le_bytes(self.take(4, what)?.try_into().unwrap())) }
    fn f64(&mut self, what: &str) -> Result<f64, String> { Ok(f64::from_le_bytes(self.take(8, what)?.try_into().unwrap())) }
}

pub fn parse(payload: &[u8]) -> Result<Record, String> {
    let mut c = Cursor { data: payload, at: 0 };
    let version = c.u8("header")?;
    if version != FMT_VERSION && version != FMT_VERSION_2 {
        return Err(format!("unsupported kind 2 format version {version}"));
    }
    let orientation = c.u8("header")?;
    if orientation != 0 {
        return Err(format!("unknown kind 2 orientation code {orientation}"));
    }
    let flags = c.u16("header")?;
    let clip_id = c.u32("header")?;
    let frame_seq = c.u32("header")?;
    let sample_period_ps = c.u32("header")?;
    let gscale = c.f32("header")?;
    let ascale = c.f32("header")?;
    let (readout_ns, mode_id, crop, sensor_mode, calib_focal_tenths_mm) = if version == FMT_VERSION {
        let readout_ns = c.u32("header")?;
        let mode_id = c.u16("header")?;
        let crop = [c.u16("header")?, c.u16("header")?, c.u16("header")?, c.u16("header")?];
        (readout_ns, mode_id, crop, None, None)
    } else {
        let mode_id = c.u16("header")?;
        let mode = SensorMode {
            hmax: c.u16("header")?, read_w: c.u16("header")?, lines: c.u16("header")?,
            bin_h: c.u16("header")?, bin_v: c.u16("header")?,
        };
        let calib = c.u16("header")?;
        // No mode running: no readout and no window, as version 1 wrote them.
        let (readout_ns, crop) = if mode_id == 0 { (0, [0; 4]) } else { (mode.readout_ns(), mode.crop()) };
        (readout_ns, mode_id, crop, Some(mode), (calib != 0).then_some(calib as u32))
    };
    let frame_mark = c.u16("header")?;
    let base_idx = c.u32("header")?;
    let n_samples = c.u16("header")? as usize;
    let n_events = c.u16("header")? as usize;
    debug_assert_eq!(c.at, HEADER_BYTES);

    if sample_period_ps == 0 {
        return Err("kind 2 sample period is zero".to_owned());
    }
    if !(gscale.is_finite() && gscale > 0.0) || !(ascale.is_finite() && ascale > 0.0) {
        return Err("kind 2 gyro/accelerometer scale is not a positive number".to_owned());
    }

    let mut samples = Vec::with_capacity(n_samples);
    for _ in 0..n_samples {
        samples.push([c.i16("samples")?, c.i16("samples")?, c.i16("samples")?]);
    }
    let mut events = Vec::with_capacity(n_events);
    for _ in 0..n_events {
        let pos = c.u16("events")?;
        let kind = c.u8("events")?;
        let len = c.u8("events")? as usize;
        let value = c.take(len, "events")?.to_vec();
        let expected = match kind {
            EVENT_LEVEL => Some(6),
            EVENT_FOCUS => Some(4),
            EVENT_VD => Some(4),
            _ => None, // unknown kinds are skipped by length
        };
        if expected.is_some_and(|n| n != len) {
            return Err(format!("kind 2 event type {kind} has length {len}"));
        }
        events.push(Event { pos, kind, value });
    }
    let lens_table = if version == FMT_VERSION && flags & FLAG_LENS_TABLE != 0 {
        let calib_focal_tenths_mm = c.u32("lens table")?;
        let mut axis = [0_u32; 5];
        for value in &mut axis { *value = c.u32("lens table")?; }
        let mut nodes = [[[0_f64; 4]; 3]; 5];
        for node in &mut nodes {
            for plane in node.iter_mut() {
                for coefficient in plane.iter_mut() { *coefficient = c.f64("lens table")?; }
            }
        }
        Some(LensTable { calib_focal_tenths_mm, axis, nodes })
    } else {
        None
    };
    if c.at != payload.len() {
        return Err(format!("kind 2 payload has {} unexpected trailing bytes", payload.len() - c.at));
    }
    Ok(Record {
        fmt_version: version, orientation, flags, clip_id, frame_seq, sample_period_ps, gscale, ascale, readout_ns,
        mode_id, crop, sensor_mode, calib_focal_tenths_mm, frame_mark, base_idx, samples, events, lens_table,
    })
}

/// Per-frame values the DNG itself already carries, read from its EXIF/DNG tags.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DngInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub software: Option<String>,
    pub lens_model: Option<String>,
    /// DefaultCropSize: the size of the developed picture.
    pub frame_size: Option<(u32, u32)>,
    pub frame_rate: Option<f64>,
    pub exposure_s: Option<f64>,
    pub iso: Option<u32>,
    pub f_number: Option<f64>,
    pub focal_length_mm: Option<f64>,
    pub subject_distance_m: Option<f64>,
    /// Green-plane kr0..kr3 of the frame's WarpRectilinear opcode: the camera's
    /// distortion, already interpolated for this frame's focus.
    pub warp_rectilinear: Option<[f64; 4]>,
}

/// Turns a take's kind 2 records, in file order, into Gyroflow Protobuf messages.
///
/// Samples are deduplicated by global index, so guard samples that two
/// neighbouring blocks share are emitted once and a dropped block leaves a gap
/// rather than a shift. The accelerometer is sample-and-hold from level events.
/// Each frame's Vd, as a gyro stream position (samples, fractional).
///
/// Vd fires once per sensor frame on the sensor's clock (+-5 us measured), so
/// all of a take's Vd positions lie on one line: index n at a + b n, b being
/// the samples per frame, fitted (it absorbs the two clocks' drift). Which Vd
/// is a frame's: the frame hook runs a steady time after its Vd, but now and
/// then tens of milliseconds late, past the next Vd. So each frame's index is
/// its frame_seq plus an offset, the offset taken as the median over nine
/// frames of what each hook suggests: a late hook is outvoted, a sensor frame
/// the recording skipped (a lasting step) is not.
pub fn vd_frame_positions(frames: &[(Record, DngInfo)], fps: f64) -> Result<Vec<f64>, String> {
    let period_s = frames.first().ok_or("no frames")?.0.sample_period_ps as f64 * 1e-12;
    if !(period_s > 0.0 && fps > 0.0) {
        return Err("no sample period or frame rate".to_owned());
    }
    let nominal = 1.0 / (fps * period_s);
    let mut vds: Vec<f64> = frames.iter()
        .flat_map(|(r, _)| r.events.iter().filter(|e| e.kind == EVENT_VD).map(move |e| r.base_idx as f64 + e.pos as f64))
        .collect();
    vds.sort_by(f64::total_cmp);
    vds.dedup();
    if vds.len() < 2 {
        return Err("this take has no Vd frame sync (it needs Gyro2 with the Vd hook)".to_owned());
    }
    let index: Vec<f64> = vds.iter().map(|v| ((v - vds[0]) / nominal).round()).collect();
    let n = vds.len() as f64;
    let (mi, mv) = (index.iter().sum::<f64>() / n, vds.iter().sum::<f64>() / n);
    let sii: f64 = index.iter().map(|i| (i - mi).powi(2)).sum();
    let siv: f64 = index.iter().zip(&vds).map(|(i, v)| (i - mi) * (v - mv)).sum();
    if !(sii > 0.0) {
        return Err("Vd frame sync events do not span a frame".to_owned());
    }
    let slope = siv / sii;
    if (slope / nominal - 1.0).abs() > 0.01 {
        return Err(format!("Vd every {slope:.2} samples does not match {fps:.3} fps ({nominal:.2})"));
    }
    let line = |i: f64| mv + slope * (i - mi);      // Vd number i, counted from the take's first

    let hooks: Vec<f64> = frames.iter().map(|(r, _)| r.base_idx as f64 + r.frame_mark as f64).collect();
    let mut lags: Vec<f64> = hooks.iter().filter_map(|h| {
        let i = ((h - line(0.0)) / slope).floor();
        (i >= 0.0).then(|| h - line(i))
    }).collect();
    if lags.is_empty() {
        return Err("no frame hook after a Vd".to_owned());
    }
    lags.sort_by(f64::total_cmp);
    let lag = lags[lags.len() / 2];
    let offsets: Vec<i64> = frames.iter().zip(&hooks)
        .map(|((r, _), h)| ((h - lag - line(0.0)) / slope).round() as i64 - r.frame_seq as i64)
        .collect();
    Ok(frames.iter().enumerate().map(|(k, (r, _))| {
        let lo = k.saturating_sub(4);
        let hi = (k + 5).min(offsets.len());
        let mut window = offsets[lo..hi].to_vec();
        window.sort_unstable();
        line((r.frame_seq as i64 + window[window.len() / 2]) as f64)
    }).collect())
}

/// What the user sets for a take, beyond what the frames carry.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Options {
    /// A lens without electronic contacts: the frames carry no focal length,
    /// so this one (mm) sizes the camera matrix, with the generic distortion
    /// of `lensfit::generic_rectilinear`.
    pub manual_focal_mm: Option<f64>,
    /// With it, the corner distortion of that lens against an ideal one (a
    /// fraction); None = the typical one for the focal length.
    pub manual_corner: Option<f64>,
}

#[allow(dead_code)] // used by tests
pub fn to_messages(frames: &[(Record, DngInfo)]) -> Result<Vec<gyroflow_proto::Main>, String> {
    to_messages_with(frames, &Options::default())
}

pub fn to_messages_with(frames: &[(Record, DngInfo)], options: &Options) -> Result<Vec<gyroflow_proto::Main>, String> {
    let (first, first_dng) = frames.first().ok_or("no kind 2 frames")?;
    let (width, height) = first_dng.frame_size
        .ok_or("first DNG has no DefaultCropSize; cannot size the clip")?;
    let fps = first_dng.frame_rate.filter(|fps| *fps > 0.0)
        .ok_or("first DNG has no FrameRate tag")?;

    for (index, (record, _)) in frames.iter().enumerate() {
        if record.clip_id != first.clip_id {
            return Err(format!("frame {} belongs to another take (clip id {:#x}, expected {:#x})", index + 1, record.clip_id, first.clip_id));
        }
        if record.sample_period_ps != first.sample_period_ps || record.gscale != first.gscale || record.ascale != first.ascale {
            return Err(format!("frame {}: sample period or scale changed within the take", index + 1));
        }
    }

    let any = |flag: u16| frames.iter().any(|(record, _)| record.flags & flag != 0);
    if any(FLAG_OIS_ON) {
        log::warn!("Lens OIS was on during this take; Gyroflow cannot undo it, so stabilisation will fight it.");
    }
    if any(FLAG_EIS_ON) {
        log::warn!("In-camera electronic stabilisation was on during this take; its warp is not recorded.");
    }
    for (index, (record, _)) in frames.iter().enumerate() {
        if record.flags & FLAG_TRUNCATED != 0 {
            log::warn!("Frame {}: the camera truncated the gyro block to fit the MakerNote.", index + 1);
        }
    }

    let period_us = first.sample_period_ps as f64 / 1.0e6;
    let gyro_dps = first.gscale as f64 * 180.0 / std::f64::consts::PI;
    let accel_ms2 = first.ascale as f64 * STANDARD_GRAVITY;
    // Accelerometer axes are a quarter turn from the gyro's: native Y is gyro
    // X, native X negated is gyro Y (fpSup gcsv_rows.S rule 2).
    let accel_in_gyro_axes = |raw: [i16; 3]| -> [f32; 3] {
        [(raw[1] as f64 * accel_ms2) as f32, (-(raw[0] as f64) * accel_ms2) as f32, (raw[2] as f64 * accel_ms2) as f32]
    };

    // Hold the first level reading of the take until the first one arrives,
    // so no sample carries an all-zero accelerometer vector.
    let mut held_accel = frames.iter()
        .flat_map(|(record, _)| record.events.iter())
        .find(|event| event.kind == EVENT_LEVEL)
        .map(|event| accel_in_gyro_axes(level_counts(event)))
        .unwrap_or([0.0; 3]);

    let header = gyroflow_proto::Header {
        camera: Some(gyroflow_proto::header::CameraMetadata {
            camera_brand: first_dng.make.clone().unwrap_or_else(|| "SIGMA".to_owned()),
            camera_model: first_dng.model.clone().unwrap_or_else(|| "fp".to_owned()),
            camera_serial_number: first_dng.serial.clone(),
            firmware_version: first_dng.software.clone(),
            lens_brand: String::new(),
            lens_model: first_dng.lens_model.clone().unwrap_or_default(),
            pixel_pitch_x_nm: PIXEL_PITCH_NM,
            pixel_pitch_y_nm: PIXEL_PITCH_NM,
            sensor_pixel_width: SENSOR_PIXEL_WIDTH,
            sensor_pixel_height: SENSOR_PIXEL_HEIGHT,
            imu_orientation: Some("xyz".to_owned()),
            ..Default::default()
        }),
        clip: Some(gyroflow_proto::header::ClipMetadata {
            frame_width: width,
            frame_height: height,
            duration_us: frames.len() as f64 * 1.0e6 / fps,
            record_frame_rate: fps as f32,
            sensor_frame_rate: fps as f32,
            file_frame_rate: fps as f32,
            imu_sample_rate: (1.0e12 / first.sample_period_ps as f64).round() as u32,
            frame_readout_time_us: first.readout_ns as f64 / 1000.0,
            frame_readout_direction: gyroflow_proto::header::clip_metadata::ReadoutDirection::TopToBottom as i32,
            pixel_aspect_ratio: 1.0,
            ..Default::default()
        }),
    };

    // The take's lens table, normally on its first frame; frames without one in
    // reach fall back to their own WarpRectilinear opcode.
    let lens_table = frames.iter().find_map(|(record, _)| record.lens_table.as_ref());
    // A zoom during the take: the table holds the focal length the take started
    // at, so it only describes the frames still there.
    let zoom = zoom_start(frames);

    let vd_positions = vd_frame_positions(frames, fps)?;
    let mut next_global: u64 = 0;
    let mut messages = Vec::with_capacity(frames.len());
    for (index, (record, dng)) in frames.iter().enumerate() {
        let base = record.base_idx as u64;
        let time_of = |position: u64| (base + position) as f64 * period_us;
        let start = vd_positions[index] * period_us + VD_TO_READOUT_US;

        // Level events in this block, by position, to hold the accelerometer.
        let mut levels: Vec<(u64, [f32; 3])> = record.events.iter()
            .filter(|event| event.kind == EVENT_LEVEL)
            .map(|event| (event.pos as u64, accel_in_gyro_axes(level_counts(event))))
            .collect();
        levels.sort_by_key(|(pos, _)| *pos);
        let mut level_iter = levels.into_iter().peekable();

        let mut imu = Vec::new();
        if record.flags & FLAG_DATA_LOST == 0 {
            for (position, raw) in record.samples.iter().enumerate() {
                let position = position as u64;
                // An event sits after the samples that preceded it: it applies
                // from the next sample on.
                while level_iter.peek().is_some_and(|(pos, _)| *pos <= position) {
                    held_accel = level_iter.next().unwrap().1;
                }
                let global = base + position;
                if global < next_global { continue; }
                next_global = global + 1;
                imu.push(gyroflow_proto::ImuData {
                    sample_timestamp_us: Some(time_of(position)),
                    gyroscope_x: (raw[0] as f64 * gyro_dps) as f32,
                    gyroscope_y: (raw[1] as f64 * gyro_dps) as f32,
                    gyroscope_z: (raw[2] as f64 * gyro_dps) as f32,
                    accelerometer_x: held_accel[0],
                    accelerometer_y: held_accel[1],
                    accelerometer_z: held_accel[2],
                    ..Default::default()
                });
            }
        }
        for (_, value) in level_iter { held_accel = value; }

        let lens = lens_data(lens_table, zoom, record, dng, width, height, options);
        let frame = gyroflow_proto::FrameMetadata {
            start_timestamp_us: start,
            end_timestamp_us: start + record.readout_ns as f64 / 1000.0,
            frame_number: u32::try_from(index + 1).map_err(|_| "too many frames".to_owned())?,
            iso: dng.iso,
            exposure_time_us: dng.exposure_s.map(|s| s * 1.0e6),
            crop_x: Some(record.crop[0] as f32),
            crop_y: Some(record.crop[1] as f32),
            crop_width: Some(record.crop[2] as f32),
            crop_height: Some(record.crop[3] as f32),
            lens: if lens == gyroflow_proto::LensData::default() { vec![] } else { vec![lens] },
            imu,
            ..Default::default()
        };
        messages.push(gyroflow_proto::Main {
            magic_string: "GyroflowProtobuf".to_owned(),
            protocol_version: 1,
            header: (index == 0).then(|| header.clone()),
            frame: Some(frame),
        });
    }
    Ok(messages)
}

/// One frame's lens: focal length, aperture, focus distance, and - when the
/// camera's correction data reaches this frame - the camera matrix and fisheye
/// coefficients fitted for this frame's focus, so breathing and distortion
/// follow a focus pull.
/// Focal lengths (mm) that differ by less than this are the same zoom position:
/// the mount reports tenths of a millimetre.
const SAME_FOCAL_MM: f64 = 0.05;

/// The focal length the mount reported on the take's first frame, when the
/// take's frames do not all report the same one (a zoom lens moved); None for
/// a take at one focal length, or without one.
pub fn zoom_start(frames: &[(Record, DngInfo)]) -> Option<f64> {
    let mut focals = frames.iter()
        .filter_map(|(_, dng)| dng.focal_length_mm)
        .filter(|mm| mm.is_finite() && *mm > 0.0);
    let start = focals.next()?;
    focals.any(|mm| (mm - start).abs() >= SAME_FOCAL_MM).then_some(start)
}

fn lens_data(table: Option<&LensTable>, zoom: Option<f64>, record: &Record, dng: &DngInfo, width: u32, height: u32, options: &Options) -> gyroflow_proto::LensData {
    let distance_mm = dng.subject_distance_m.map(|m| m * 1000.0);
    // No focal length from the mount: a lens without electronic contacts. The
    // camera's correction table may still be the last electronic lens's, so it
    // and the opcode are not used; the user's focal length is, or nothing.
    if !dng.focal_length_mm.is_some_and(|mm| mm.is_finite() && mm > 0.0) {
        let mut lens = gyroflow_proto::LensData {
            f_number: dng.f_number.map(|v| v as f32),
            ..Default::default()
        };
        if let Some(mm) = options.manual_focal_mm.filter(|mm| mm.is_finite() && *mm > 0.0) {
            let fx = lensfit::focal_px(width, mm, record.crop[2]) as f32;
            lens.focal_length_mm = Some(mm as f32);
            lens.camera_intrinsic_matrix = vec![fx, 0.0, width as f32 / 2.0, 0.0, fx, height as f32 / 2.0, 0.0, 0.0, 1.0];
            let k = lensfit::generic_rectilinear(mm, width, height, fx as f64, options.manual_corner);
            lens.distortion = Some(gyroflow_proto::lens_data::Distortion::OpencvFisheye(gyroflow_proto::OpenCvFisheye {
                coefficients: k.iter().map(|v| *v as f32).collect(),
            }));
        }
        return lens;
    }
    // In a zoomed take, a frame away from the starting focal length takes its
    // own: the mount's number and the camera's opcode written for that frame.
    // The calibrated focal length is the one of the take's start (Gyro1 read it
    // there), so the same rule holds it to the frames still at that focal length.
    let unzoomed = zoom.is_none_or(|start| dng.focal_length_mm.is_some_and(|mm| (mm - start).abs() < SAME_FOCAL_MM));
    let table = table.filter(|_| unzoomed);
    let (kr, focal_mm) = match table {
        Some(table) => (
            lensfit::interpolate(&table.axis, &table.nodes, distance_mm),
            lensfit::focal_mm(Some(table.calib_focal_tenths_mm), dng.focal_length_mm),
        ),
        None => (dng.warp_rectilinear, lensfit::focal_mm(record.calib_focal_tenths_mm.filter(|_| unzoomed), dng.focal_length_mm)),
    };
    let mut lens = gyroflow_proto::LensData {
        focal_length_mm: focal_mm.or(dng.focal_length_mm).map(|v| v as f32),
        f_number: dng.f_number.map(|v| v as f32),
        focus_distance_mm: distance_mm.map(|mm| mm as f32),
        ..Default::default()
    };
    let fitted = focal_mm.zip(kr).and_then(|(focal_mm, kr)| {
        lensfit::fit(kr, width, height, lensfit::focal_px(width, focal_mm, record.crop[2]))
    });
    if let Some((coefficients, fx)) = fitted {
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let fx = fx as f32;
        lens.camera_intrinsic_matrix = vec![fx, 0.0, cx, 0.0, fx, cy, 0.0, 0.0, 1.0];
        lens.distortion = Some(gyroflow_proto::lens_data::Distortion::OpencvFisheye(gyroflow_proto::OpenCvFisheye {
            coefficients: coefficients.iter().map(|v| *v as f32).collect(),
        }));
    }
    lens
}

fn level_counts(event: &Event) -> [i16; 3] {
    let v = &event.value;
    [i16::from_le_bytes([v[0], v[1]]), i16::from_le_bytes([v[2], v[3]]), i16::from_le_bytes([v[4], v[5]])]
}

/// Reference encoder: the byte layout the camera writes. Used by the tests and
/// kept next to the decoder so the two cannot drift apart.
#[cfg_attr(not(test), allow(dead_code))]
pub fn encode(record: &Record) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + record.samples.len() * 6);
    out.push(record.fmt_version);
    out.push(record.orientation);
    out.extend_from_slice(&record.flags.to_le_bytes());
    out.extend_from_slice(&record.clip_id.to_le_bytes());
    out.extend_from_slice(&record.frame_seq.to_le_bytes());
    out.extend_from_slice(&record.sample_period_ps.to_le_bytes());
    out.extend_from_slice(&record.gscale.to_le_bytes());
    out.extend_from_slice(&record.ascale.to_le_bytes());
    if record.fmt_version == FMT_VERSION {
        out.extend_from_slice(&record.readout_ns.to_le_bytes());
        out.extend_from_slice(&record.mode_id.to_le_bytes());
        for v in record.crop { out.extend_from_slice(&v.to_le_bytes()); }
    } else {
        let mode = record.sensor_mode.expect("a version 2 record has a sensor mode");
        out.extend_from_slice(&record.mode_id.to_le_bytes());
        for v in [mode.hmax, mode.read_w, mode.lines, mode.bin_h, mode.bin_v] { out.extend_from_slice(&v.to_le_bytes()); }
        out.extend_from_slice(&(record.calib_focal_tenths_mm.unwrap_or(0) as u16).to_le_bytes());
    }
    out.extend_from_slice(&record.frame_mark.to_le_bytes());
    out.extend_from_slice(&record.base_idx.to_le_bytes());
    out.extend_from_slice(&(record.samples.len() as u16).to_le_bytes());
    out.extend_from_slice(&(record.events.len() as u16).to_le_bytes());
    for sample in &record.samples {
        for v in sample { out.extend_from_slice(&v.to_le_bytes()); }
    }
    for event in &record.events {
        out.extend_from_slice(&event.pos.to_le_bytes());
        out.push(event.kind);
        out.push(event.value.len() as u8);
        out.extend_from_slice(&event.value);
    }
    if let Some(table) = record.lens_table.as_ref().filter(|_| record.fmt_version == FMT_VERSION) {
        out.extend_from_slice(&table.calib_focal_tenths_mm.to_le_bytes());
        for v in table.axis { out.extend_from_slice(&v.to_le_bytes()); }
        for node in &table.nodes {
            for plane in node {
                for v in plane { out.extend_from_slice(&v.to_le_bytes()); }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD_PS: u32 = 400_085_400;
    const GSCALE: f32 = 0.000137923;
    const ASCALE: f32 = 0.0009765625;

    pub fn record(base_idx: u32, frame_seq: u32, samples: Vec<[i16; 3]>) -> Record {
        Record {
            fmt_version: FMT_VERSION,
            orientation: 0,
            flags: 0,
            clip_id: 0x1234_5678,
            frame_seq,
            sample_period_ps: PERIOD_PS,
            gscale: GSCALE,
            ascale: ASCALE,
            readout_ns: 21_325_000,
            mode_id: 123,
            crop: [64, 1840, 5872, 3304],
            sensor_mode: None,
            calib_focal_tenths_mm: None,
            frame_mark: 2,
            base_idx,
            samples,
            events: vec![],
            lens_table: None,
        }
    }

    /// The same record as the camera writes it since version 2: mode 123,
    /// 5872 x 3304 read 1:1 at 4.6 us a line.
    pub fn record_v2(base_idx: u32, frame_seq: u32, samples: Vec<[i16; 3]>) -> Record {
        let mode = SensorMode { hmax: 333, read_w: 5872, lines: 3304, bin_h: 1, bin_v: 1 };
        Record {
            fmt_version: FMT_VERSION_2,
            readout_ns: mode.readout_ns(),
            crop: mode.crop(),
            sensor_mode: Some(mode),
            ..record(base_idx, frame_seq, samples)
        }
    }

    fn level(pos: u16, raw: [i16; 3]) -> Event {
        Event { pos, kind: EVENT_LEVEL, value: raw.iter().flat_map(|v| v.to_le_bytes()).collect() }
    }

    /// A take's frames with a Vd stream: Vd every nominal frame from the first
    /// frame's base (all carried in the first block; only positions count).
    pub fn with_vd(mut frames: Vec<(Record, DngInfo)>) -> Vec<(Record, DngInfo)> {
        let per_frame = 1.0 / (30000.0 / 1001.0 * PERIOD_PS as f64 * 1e-12);
        let count = frames.len() + 2;
        let first = &mut frames[0].0;
        for n in 0..count {
            let pos = (n as f64 * per_frame).round() as u16;
            first.events.push(Event { pos, kind: EVENT_VD, value: (n as u32 * 33_367).to_le_bytes().to_vec() });
        }
        frames
    }

    fn dng() -> DngInfo {
        DngInfo {
            make: Some("SIGMA".into()),
            model: Some("SIGMA fp".into()),
            lens_model: Some("28mm F1.4 DG HSM | Art 019".into()),
            frame_size: Some((1920, 1080)),
            frame_rate: Some(30000.0 / 1001.0),
            exposure_s: Some(1.0 / 60.0),
            iso: Some(100),
            f_number: Some(1.4),
            focal_length_mm: Some(28.0),
            subject_distance_m: Some(0.403),
            ..Default::default()
        }
    }

    fn lens_table() -> LensTable {
        let mut nodes = [[[0.0; 4]; 3]; 5];
        for (n, node) in nodes.iter_mut().enumerate() {
            for (p, plane) in node.iter_mut().enumerate() {
                *plane = [1.0 + n as f64 * 1e-3, -0.0148 + p as f64 * 1e-4, 0.0052, -0.0098];
            }
        }
        LensTable { calib_focal_tenths_mm: 394, axis: [0, 18641, 37283, 49637, 55924], nodes }
    }

    #[test]
    fn encode_parse_round_trip_with_events_and_lens_table() {
        let mut original = record(1000, 7, vec![[1, -2, 3], [-32768, 32767, 0]]);
        original.flags = FLAG_FIRST_FRAME | FLAG_LENS_TABLE;
        original.events = vec![level(1, [10, -20, 1024]), Event { pos: 0, kind: EVENT_FOCUS, value: 9000_u32.to_le_bytes().to_vec() }];
        original.lens_table = Some(lens_table());
        let bytes = encode(&original);
        assert_eq!(bytes.len(), HEADER_BYTES + 2 * 6 + (4 + 6) + (4 + 4) + LENS_TABLE_BYTES);
        assert_eq!(parse(&bytes).unwrap(), original);
    }

    #[test]
    fn header_offsets_match_the_spec() {
        let bytes = encode(&record(0xAABB_CCDD, 0, vec![]));
        assert_eq!(&bytes[12..16], &PERIOD_PS.to_le_bytes());
        assert_eq!(&bytes[16..20], &GSCALE.to_le_bytes());
        assert_eq!(&bytes[24..28], &21_325_000_u32.to_le_bytes());
        assert_eq!(&bytes[38..40], &2_u16.to_le_bytes());
        assert_eq!(&bytes[40..44], &0xAABB_CCDD_u32.to_le_bytes());
        assert_eq!(bytes.len(), HEADER_BYTES);
    }

    #[test]
    fn version_2_round_trip_and_offsets() {
        let mut original = record_v2(0xAABB_CCDD, 7, vec![[1, -2, 3]]);
        original.calib_focal_tenths_mm = Some(394);
        original.events = vec![level(0, [10, -20, 1024])];
        let bytes = encode(&original);
        assert_eq!(bytes.len(), HEADER_BYTES + 6 + 10);
        assert_eq!(bytes[0], 2);
        assert_eq!(&bytes[24..38], &[123, 0, 77, 1, 0xF0, 0x16, 0xE8, 0x0C, 1, 0, 1, 0, 0x8A, 1]);
        assert_eq!(&bytes[38..40], &2_u16.to_le_bytes());
        assert_eq!(&bytes[40..44], &0xAABB_CCDD_u32.to_le_bytes());
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed, original);
        assert_eq!(parsed.crop, [64, 348, 5872, 3304]);
        assert_eq!(parsed.readout_ns, 15_281_000);
    }

    #[test]
    fn version_2_derives_the_window_like_gyro1() {
        // Mode 106 (Gyro1's A001_040): 3000 x 2000 read at 2 x 2, hmax 380.
        let mode = SensorMode { hmax: 380, read_w: 3000, lines: 2000, bin_h: 2, bin_v: 2 };
        assert_eq!(mode.readout_ns(), 10_555_556);
        assert_eq!(mode.crop(), [0, 0, 6000, 4000]);
        // Binning 0 is 1; more than the sensor is the sensor.
        let odd = SensorMode { hmax: 1, read_w: 7000, lines: 100, bin_h: 0, bin_v: 0 };
        assert_eq!(odd.crop(), [0, 1950, 6000, 100]);
        // No mode running: version 1 wrote zeros there, and so does the reader.
        let mut none = record_v2(0, 0, vec![]);
        none.mode_id = 0;
        let parsed = parse(&encode(&none)).unwrap();
        assert_eq!((parsed.readout_ns, parsed.crop), (0, [0; 4]));
    }

    #[test]
    fn version_2_has_no_lens_table_even_with_the_flag() {
        let mut flagged = record_v2(0, 0, vec![]);
        flagged.flags = FLAG_LENS_TABLE;
        assert_eq!(parse(&encode(&flagged)).unwrap().lens_table, None);
    }

    #[test]
    fn unknown_event_types_are_skipped_by_length() {
        let mut original = record(0, 0, vec![[1, 2, 3]]);
        original.events = vec![Event { pos: 0, kind: 77, value: vec![1, 2, 3, 4, 5, 6, 7] }, level(0, [0, 0, 1024])];
        assert_eq!(parse(&encode(&original)).unwrap().events.len(), 2);
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        let good = encode(&record(0, 0, vec![[1, 2, 3]]));
        let mut bad_version = good.clone();
        bad_version[0] = 9;
        assert!(parse(&bad_version).unwrap_err().contains("format version 9"));
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(parse(&trailing).unwrap_err().contains("trailing"));
        assert!(parse(&good[..good.len() - 1]).unwrap_err().contains("truncated in samples"));
        let mut wrong_level = record(0, 0, vec![]);
        wrong_level.events = vec![Event { pos: 0, kind: EVENT_LEVEL, value: vec![0; 4] }];
        assert!(parse(&encode(&wrong_level)).unwrap_err().contains("type 1 has length 4"));
        let mut no_table = record(0, 0, vec![]);
        no_table.flags = FLAG_LENS_TABLE;
        assert!(parse(&encode(&no_table)).unwrap_err().contains("truncated in lens table"));
        let mut zero_period = record(0, 0, vec![]);
        zero_period.sample_period_ps = 0;
        assert!(parse(&encode(&zero_period)).unwrap_err().contains("period is zero"));
    }

    #[test]
    fn each_frame_takes_its_own_vd_through_late_hooks_and_skipped_frames() {
        let fps = 30000.0 / 1001.0;
        let per_frame = 1.0 / (fps * PERIOD_PS as f64 * 1e-12);       // 83.40
        let vd = |n: f64| 1000.0 + n * per_frame * (1.0 + 2e-3);     // the clocks drift (exaggerated: 0.2 %)
        // 40 frames; sensor frame 25 was skipped by the recording (frame_seq goes on).
        let sensor: Vec<f64> = (0..40).map(|k| if k < 25 { k as f64 } else { k as f64 + 1.0 }).collect();
        let lag = [35.0, 34.0, 36.0, 35.0, 35.0];
        let hooks: Vec<u32> = sensor.iter().enumerate().map(|(k, n)| {
            (vd(*n) + lag[k % 5] + if k == 12 { 60.0 } else { 0.0 }).round() as u32   // frame 12: late, past the next Vd
        }).collect();
        // Every sensor frame's Vd fired (the skipped one's too), but some never
        // reached a block; each lands in the block whose span holds it, as on the camera.
        let vds: Vec<u32> = (0..41).filter(|n| n % 7 != 3).map(|n| vd(n as f64).round() as u32).collect();
        let mut frames = Vec::new();
        let mut start = 900u32;
        for (k, hook) in hooks.iter().enumerate() {
            let mut r = record(start, k as u32, vec![]);
            r.frame_mark = (hook - start) as u16;
            for v in vds.iter().filter(|v| **v >= start && **v < *hook) {
                r.events.push(Event { pos: (v - start) as u16, kind: EVENT_VD, value: vec![0; 4] });
            }
            start = *hook;
            frames.push((r, dng()));
        }
        let got = vd_frame_positions(&frames, fps).unwrap();
        for (k, n) in sensor.iter().enumerate() {
            assert!((got[k] - vd(*n)).abs() < 0.6, "frame {k}: {} vs {}", got[k], vd(*n));
        }
        // Without Vd the take is refused.
        for (r, _) in &mut frames { r.events.clear(); }
        assert!(vd_frame_positions(&frames, fps).unwrap_err().contains("no Vd"));
    }

    #[test]
    fn a_lens_without_contacts_uses_the_manual_focal_or_nothing() {
        // No focal length in the EXIF, but a stale table from the last electronic lens.
        let mut first = record(0, 0, vec![[0, 0, 0]; 4]);
        first.lens_table = Some(lumix_40_table());
        let manual = DngInfo { frame_size: Some((1920, 1080)), focal_length_mm: None, ..dng() };
        let frames = with_vd(vec![(first, manual.clone())]);
        let lens = |options: &Options| to_messages_with(&frames, options).unwrap()[0].frame.as_ref().unwrap().lens.clone();
        assert!(lens(&Options::default()).iter().all(|l| l.camera_intrinsic_matrix.is_empty()), "the stale table is not used");
        let l = lens(&Options { manual_focal_mm: Some(50.0), ..Default::default() });
        let fx = l[0].camera_intrinsic_matrix[0];
        let want = lensfit::focal_px(1920, 50.0, frames[0].0.crop[2]) as f32;
        assert!((fx - want).abs() < 1e-3, "{fx} vs {want}");
        assert_eq!(l[0].focal_length_mm, Some(50.0));
        assert_eq!(fisheye(&l[0]), lensfit::generic_rectilinear(50.0, 1920, 1080, want as f64, None).iter().map(|v| *v as f32).collect::<Vec<_>>());
        let l = lens(&Options { manual_focal_mm: Some(50.0), manual_corner: Some(-0.1) });
        assert_eq!(fisheye(&l[0]), lensfit::generic_rectilinear(50.0, 1920, 1080, want as f64, Some(-0.1)).iter().map(|v| *v as f32).collect::<Vec<_>>());
        // An electronic lens ignores the manual focal.
        let electronic = with_vd(vec![(record(0, 0, vec![[0, 0, 0]; 4]), DngInfo { focal_length_mm: Some(28.0), ..manual })]);
        let l = to_messages_with(&electronic, &Options { manual_focal_mm: Some(50.0), ..Default::default() }).unwrap()[0].frame.as_ref().unwrap().lens.clone();
        assert_eq!(l[0].focal_length_mm, Some(28.0));
    }

    #[test]
    fn timing_and_units_come_from_positions_and_scales() {
        let frames = with_vd(vec![(record(100, 0, vec![[131, 0, -131]; 4]), dng())]);
        let messages = to_messages(&frames).unwrap();
        let frame = messages[0].frame.as_ref().unwrap();
        let period_us = PERIOD_PS as f64 / 1e6;
        // the frame's Vd is at 100 (the first of the stream), its hook 2 later
        assert!((frame.start_timestamp_us - (100.0 * period_us + VD_TO_READOUT_US)).abs() < 0.5 * period_us);
        assert!((frame.end_timestamp_us - frame.start_timestamp_us - 21_325.0).abs() < 1e-6);
        assert!((frame.imu[3].sample_timestamp_us.unwrap() - 103.0 * period_us).abs() < 1e-6);
        let dps = 131.0 * GSCALE as f64 * 180.0 / std::f64::consts::PI;
        assert!((frame.imu[0].gyroscope_x as f64 - dps).abs() < 1e-4);
        assert!((frame.imu[0].gyroscope_z as f64 + dps).abs() < 1e-4);
        assert_eq!(frame.exposure_time_us.map(|v| v.round()), Some(16667.0));
        let header = messages[0].header.as_ref().unwrap();
        let clip = header.clip.as_ref().unwrap();
        assert_eq!((clip.frame_width, clip.frame_height, clip.imu_sample_rate), (1920, 1080, 2499));
        assert_eq!(header.camera.as_ref().unwrap().imu_orientation.as_deref(), Some("xyz"));
    }

    #[test]
    fn overlapping_guards_are_emitted_once_and_a_lost_block_leaves_a_gap() {
        // Frame 1 covers 0..10, frame 2 re-sends 8..9 as guards and runs to 15,
        // frame 3 lost its data, frame 4 resumes at 30.
        let mut lost = record(15, 2, vec![]);
        lost.flags = FLAG_DATA_LOST;
        let frames = vec![
            (record(0, 0, vec![[1, 0, 0]; 10]), dng()),
            (record(8, 1, vec![[2, 0, 0]; 7]), dng()),
            (lost, dng()),
            (record(30, 3, vec![[3, 0, 0]; 5]), dng()),
        ];
        let messages = to_messages(&with_vd(frames)).unwrap();
        let counts: Vec<usize> = messages.iter().map(|m| m.frame.as_ref().unwrap().imu.len()).collect();
        assert_eq!(counts, vec![10, 5, 0, 5]);
        let period_us = PERIOD_PS as f64 / 1e6;
        let first_of_frame_2 = messages[1].frame.as_ref().unwrap().imu[0].sample_timestamp_us.unwrap();
        assert!((first_of_frame_2 - 10.0 * period_us).abs() < 1e-6);
        let start = |k: usize| messages[k].frame.as_ref().unwrap().start_timestamp_us;
        assert!(start(1) < start(2) && start(2) < start(3), "a lost block still moves the timeline on");
        assert!(messages.iter().skip(1).all(|m| m.header.is_none()));
    }

    #[test]
    fn accelerometer_is_held_and_turned_into_gyro_axes() {
        let mut first = record(0, 0, vec![[0, 0, 0]; 4]);
        first.events = vec![level(2, [100, 200, 1024])];
        let mut second = record(4, 1, vec![[0, 0, 0]; 2]);
        second.events = vec![level(1, [-50, 0, 1000])];
        let messages = to_messages(&with_vd(vec![(first, dng()), (second, dng())])).unwrap();
        let g = ASCALE as f64 * STANDARD_GRAVITY;
        let acc = |m: usize, i: usize| {
            let s = &messages[m].frame.as_ref().unwrap().imu[i];
            [s.accelerometer_x as f64, s.accelerometer_y as f64, s.accelerometer_z as f64]
        };
        // Before the take's first reading, that reading is held backwards.
        let first_reading = [200.0 * g, -100.0 * g, 1024.0 * g];
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        assert!(close(acc(0, 0), first_reading));
        assert!(close(acc(0, 3), first_reading));
        assert!(close(acc(1, 0), first_reading));
        assert!(close(acc(1, 1), [0.0, 50.0 * g, 1000.0 * g]));
    }

    // The LUMIX S 40/F2's green plane (fpSup gyro/test_distfit.py).
    fn lumix_40_table() -> LensTable {
        let green = [
            [0.987878098, 0.000230792, -0.001804660, 0.012710940],
            [0.995612771, -0.003803673, -0.003186222, 0.011056404],
            [0.999854447, -0.010596089, 0.003693395, 0.003874420],
            [0.999856513, -0.016680599, 0.013089761, -0.003420052],
            [0.999845119, -0.020184787, 0.018855619, -0.007281209],
        ];
        LensTable { calib_focal_tenths_mm: 400, axis: [0, 18641, 37283, 49637, 55924], nodes: green.map(|kr| [kr, kr, kr]) }
    }

    fn lens_of(message: &gyroflow_proto::Main) -> &gyroflow_proto::LensData {
        &message.frame.as_ref().unwrap().lens[0]
    }

    fn fisheye(lens: &gyroflow_proto::LensData) -> Vec<f32> {
        match &lens.distortion {
            Some(gyroflow_proto::lens_data::Distortion::OpencvFisheye(c)) => c.coefficients.clone(),
            other => panic!("expected OpenCV fisheye, got {other:?}"),
        }
    }

    #[test]
    fn lens_follows_a_focus_pull_from_the_first_frames_table() {
        let mut first = record(0, 0, vec![[0, 0, 0]]);
        first.flags |= FLAG_LENS_TABLE;
        first.lens_table = Some(lumix_40_table());
        first.crop[2] = 6000;
        let mut second = record(1, 1, vec![[0, 0, 0]]);
        second.crop[2] = 6000;
        let at = |metres: Option<f64>| DngInfo { frame_size: Some((1936, 1090)), focal_length_mm: Some(40.0), subject_distance_m: metres, ..dng() };
        let messages = to_messages(&with_vd(vec![(first, at(None)), (second, at(Some(0.364)))])).unwrap();

        // Golden values from fpSup's Python mirror of the camera's fit.
        let infinity = lens_of(&messages[0]);
        assert!((infinity.camera_intrinsic_matrix[0] as f64 - 2130.954872120334).abs() < 1e-2);
        assert_eq!(infinity.camera_intrinsic_matrix[0], infinity.camera_intrinsic_matrix[4]);
        assert_eq!((infinity.camera_intrinsic_matrix[2], infinity.camera_intrinsic_matrix[5]), (968.0, 545.0));
        assert!((fisheye(infinity)[0] as f64 - 0.3434034356654717).abs() < 1e-6);
        assert_eq!(infinity.focal_length_mm, Some(40.0));

        let near = lens_of(&messages[1]);
        assert!((near.camera_intrinsic_matrix[0] as f64 - 2156.792268728652).abs() < 1e-2);
        assert!((fisheye(near)[0] as f64 - 0.2772084741693018).abs() < 1e-6);
        assert!((near.focus_distance_mm.unwrap() - 364.0).abs() < 1e-3);
    }

    #[test]
    fn without_a_table_each_frame_uses_its_own_warp_opcode() {
        let opcode = DngInfo { warp_rectilinear: Some([1.000334, -0.01484, 0.00518, -0.009761]), ..dng() };
        let messages = to_messages(&with_vd(vec![(record(0, 0, vec![]), opcode)])).unwrap();
        let lens = lens_of(&messages[0]);
        assert_eq!(fisheye(lens).len(), 4);
        // The breathing term rides on the 28 mm focal: 1920 x 28 / 35.9 x kr0.
        let focal = 1920.0 * 28.0 / (35.9 * 5872.0 / 6000.0) * 1.000334;
        assert!((lens.camera_intrinsic_matrix[0] as f64 - focal).abs() < 1e-2);

        let plain = to_messages(&with_vd(vec![(record(0, 0, vec![]), dng())])).unwrap();
        let lens = lens_of(&plain[0]);
        assert!(lens.distortion.is_none() && lens.camera_intrinsic_matrix.is_empty());
        assert_eq!(lens.focal_length_mm, Some(28.0));
    }

    #[test]
    fn version_2_uses_each_frames_opcode_and_calibrated_focal_length() {
        let opcode = [1.000334, -0.01484, 0.00518, -0.009761];
        let at = |mm: f64| DngInfo { frame_size: Some((1920, 1080)), focal_length_mm: Some(mm), warp_rectilinear: Some(opcode), ..dng() };
        let mut frames = Vec::new();
        for (n, mm) in [40.0, 40.0, 60.0].into_iter().enumerate() {
            let mut r = record_v2(n as u32, n as u32, vec![[0, 0, 0]]);
            r.calib_focal_tenths_mm = Some(394);
            frames.push((r, at(mm)));
        }
        let messages = to_messages(&with_vd(frames)).unwrap();
        let fx = |k: usize| lens_of(&messages[k]).camera_intrinsic_matrix[0] as f64;
        // 39.4 mm, not the 40.0 the mount reports, over the 5872 columns read.
        let want = 1920.0 * 39.4 / (35.9 * 5872.0 / 6000.0) * 1.000334;
        assert!((fx(0) - want).abs() < 1e-2, "{} vs {want}", fx(0));
        assert_eq!(lens_of(&messages[0]).focal_length_mm, Some(39.4));
        assert!((fx(1) - want).abs() < 1e-2);
        // Zoomed away from the take's start: the mount's number.
        assert_eq!(lens_of(&messages[2]).focal_length_mm, Some(60.0));
        let zoomed = 1920.0 * 60.0 / (35.9 * 5872.0 / 6000.0) * 1.000334;
        assert!((fx(2) - zoomed).abs() < 1e-2);
    }

    #[test]
    fn version_2_blocks_that_end_before_their_mark_still_tile_the_stream() {
        // A take's first block carries the oldest 4 of its 6 samples; the
        // next carries the other 2 and its own 3.
        let mut first = record_v2(0, 0, vec![[1, 0, 0]; 4]);
        first.frame_mark = 6;
        let mut second = record_v2(4, 1, vec![[2, 0, 0]; 5]);
        second.frame_mark = 5;
        let frames = with_vd(vec![(first, dng()), (second, dng())]);
        let messages = to_messages(&frames).unwrap();
        let stamps: Vec<f64> = messages.iter().flat_map(|m| m.frame.as_ref().unwrap().imu.iter().map(|s| s.sample_timestamp_us.unwrap())).collect();
        let period_us = PERIOD_PS as f64 / 1e6;
        assert_eq!(stamps.len(), 9);
        for (k, t) in stamps.iter().enumerate() {
            assert!((t - k as f64 * period_us).abs() < 1e-6);
        }
    }

    #[test]
    fn lens_follows_a_zoom_away_from_the_tables_focal_length() {
        let opcode = [1.000334, -0.01484, 0.00518, -0.009761];
        let mut first = record(0, 0, vec![[0, 0, 0]]);
        first.flags |= FLAG_LENS_TABLE;
        first.lens_table = Some(lumix_40_table());
        first.crop[2] = 6000;
        let mut frames = vec![first];
        for n in 1..3 {
            let mut next = record(n, n, vec![[0, 0, 0]]);
            next.crop[2] = 6000;
            frames.push(next);
        }
        let at = |mm: f64| DngInfo { frame_size: Some((1936, 1090)), focal_length_mm: Some(mm), subject_distance_m: None, warp_rectilinear: Some(opcode), ..dng() };
        let frames: Vec<_> = frames.into_iter().zip([at(40.0), at(60.0), at(40.0)]).collect();
        let messages = to_messages(&with_vd(frames)).unwrap();

        // At the take's starting focal length: the table, as without a zoom.
        for index in [0, 2] {
            let lens = lens_of(&messages[index]);
            assert!((lens.camera_intrinsic_matrix[0] as f64 - 2130.954872120334).abs() < 1e-2, "frame {index}");
            assert!((fisheye(lens)[0] as f64 - 0.3434034356654717).abs() < 1e-6, "frame {index}");
        }
        // Zoomed to 60 mm: the mount's focal length and the frame's own opcode.
        let zoomed = lens_of(&messages[1]);
        assert_eq!(zoomed.focal_length_mm, Some(60.0));
        let focal = 1936.0 * 60.0 / 35.9 * 1.000334;
        assert!((zoomed.camera_intrinsic_matrix[0] as f64 - focal).abs() < 1e-2);
        assert_eq!(zoomed.camera_intrinsic_matrix[0], zoomed.camera_intrinsic_matrix[4]);
    }

    #[test]
    fn a_take_at_one_focal_length_is_not_a_zoom() {
        let take = |mm: [f64; 3]| -> Vec<(Record, DngInfo)> {
            mm.iter().enumerate().map(|(n, mm)| (record(n as u32, n as u32, vec![]), DngInfo { focal_length_mm: Some(*mm), ..dng() })).collect()
        };
        assert_eq!(zoom_start(&take([28.0, 28.0, 28.0])), None);
        assert_eq!(zoom_start(&take([28.0, 28.0, 28.02])), None);
        assert_eq!(zoom_start(&take([24.0, 24.0, 35.0])), Some(24.0));
        let manual: Vec<_> = (0..3).map(|n| (record(n, n, vec![]), DngInfo { focal_length_mm: None, ..dng() })).collect();
        assert_eq!(zoom_start(&manual), None);
    }

    #[test]
    fn a_take_cannot_mix_clips() {
        let mut other = record(10, 1, vec![]);
        other.clip_id = 1;
        let error = to_messages(&[(record(0, 0, vec![]), dng()), (other, dng())]).unwrap_err();
        assert!(error.contains("another take"), "{error}");
    }

    #[test]
    fn frame_size_and_rate_are_required_from_the_dng() {
        let mut no_rate = dng();
        no_rate.frame_rate = None;
        assert!(to_messages(&[(record(0, 0, vec![]), no_rate)]).unwrap_err().contains("FrameRate"));
        let mut no_size = dng();
        no_size.frame_size = None;
        assert!(to_messages(&[(record(0, 0, vec![]), no_size)]).unwrap_err().contains("DefaultCropSize"));
    }
}
