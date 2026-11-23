//! Experimental CinemaDNG carrier for Gyroflow Protobuf telemetry.
//!
//! Each DNG frame carries one serialized `gyroflow_proto::Main` message, either
//! in a framed block inside the unused tail of the MakerNote (the carrier the
//! SIGMA fp writer uses) or in the older prototype IFD0 tag 65000. Both are
//! local conventions, not CinemaDNG or DNG standards. Only TIFF metadata and
//! the bounded payload are read; image strips/tiles are never decoded.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use gyroflow_core::telemetry_parser::gyroflow::{gyroflow_proto, proto_json};
use prost::Message;

use crate::fsg2;

const PROTOTYPE_TAG: u16 = 65000;
const EXIF_IFD_TAG: u16 = 0x8769;
const MAKER_NOTE_TAG: u16 = 0x927C;
const OPCODE_LIST3_TAG: u16 = 0xC74E;
/// Magic of the MakerNote telemetry block. Deliberately not
/// "FPG2", which sigma-fp-supmod already uses in the same place with a
/// different layout.
const MAKER_NOTE_MAGIC: &[u8; 4] = b"FSG2";
const BLOCK_VERSION: u8 = 1;
/// A block in its own MakerNote tag: no CRC (see `parse_tag_block`).
const BLOCK_VERSION_2: u8 = 2;
/// The SIGMA MakerNote tag the fp's Gyro2 adds at record start to hold its
/// block: UNDEFINED, sized for the take. SIGMA's own tags end at 0x0200.
const GYRO2_MAKER_NOTE_TAG: u16 = 0x4653;
/// What a SIGMA MakerNote starts with; its IFD follows at +10, with offsets
/// from the start of the file.
const SIGMA_MAKER_NOTE_ID: &[u8; 8] = b"SIGMA\0\0\0";
const SIGMA_MAKER_NOTE_IFD_AT: u64 = 10;
const BLOCK_KIND_PROTOBUF: u8 = 1;
const BLOCK_KIND_GYRO2: u8 = 2;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const BLOCK_HEADER_BYTES: usize = 12;
const MAX_MAKER_NOTE_BYTES: usize = 1024 * 1024;
const MAX_PROTO_BYTES: usize = 4 * 1024 * 1024;
const MAX_JSONL_BYTES: usize = 512 * 1024 * 1024;
const MAX_FRAMES: usize = 1_000_000;
const MAX_IFD_ENTRIES: u64 = 4096;
const MAX_DIMENSION: u32 = 16_384;
const MAX_PIXELS: u64 = 100_000_000;
const MAX_FPS: f64 = 1000.0;

/// The telemetry stream and clip properties recovered from a numeric DNG sequence.
#[derive(Debug)]
pub struct SequenceData {
    /// Canonical Gyroflow Protobuf JSONL, with one `Main` record per frame.
    pub jsonl: Vec<u8>,
    pub frame_count: usize,
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    /// Clockwise image rotation in degrees, normalized to 0, 90, 180 or 270.
    pub rotation: i32,
    /// No frame has a camera matrix: a lens without electronic contacts and
    /// no manual focal length. Gyroflow then needs a lens profile.
    pub lens_missing: bool,
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn u16(self, bytes: &[u8]) -> u16 {
        let bytes: [u8; 2] = bytes.try_into().expect("two bytes");
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: &[u8]) -> u32 {
        let bytes: [u8; 4] = bytes.try_into().expect("four bytes");
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }
}

/// Read a numbered DNG sequence containing `path`.
///
/// Each file must have a numeric suffix before `.dng`, the suffixes must be
/// consecutive, and every frame must contain the experimental private tag.
/// This entry point intentionally does not search other IFDs or infer telemetry
/// from ordinary CDNG metadata.
#[allow(dead_code)] // used by tests
pub fn read_embedded_protobuf_sequence(path: &Path) -> Result<SequenceData, String> {
    read_embedded_protobuf_sequence_with(path, &fsg2::Options::default())
}

pub fn read_embedded_protobuf_sequence_with(path: &Path, options: &fsg2::Options) -> Result<SequenceData, String> {
    let paths = sequence_paths(path)?;
    let mut jsonl = Vec::new();
    let mut expected_header: Option<gyroflow_proto::Header> = None;
    let mut clip_properties = None;

    let messages = read_messages(&paths, options)?;
    let lens_missing = !messages.iter().any(|main| main.frame.as_ref()
        .is_some_and(|frame| frame.lens.iter().any(|lens| !lens.camera_intrinsic_matrix.is_empty())));
    for (index, (frame_path, main)) in paths.iter().zip(messages).enumerate() {

        if main.magic_string != "GyroflowProtobuf" {
            return Err(format!("{}: invalid Gyroflow Protobuf magic", frame_path.display()));
        }
        if main.protocol_version != 1 {
            return Err(format!("{}: unsupported Gyroflow Protobuf version {}", frame_path.display(), main.protocol_version));
        }

        if index == 0 {
            let header = main.header.as_ref()
                .ok_or_else(|| format!("{}: first frame has no Protobuf header", frame_path.display()))?;
            if header.camera.is_none() {
                return Err(format!("{}: Protobuf header has no camera metadata", frame_path.display()));
            }
            let clip = header.clip.as_ref()
                .ok_or_else(|| format!("{}: Protobuf header has no clip metadata", frame_path.display()))?;
            if clip.frame_width == 0 || clip.frame_height == 0
                || clip.frame_width > MAX_DIMENSION || clip.frame_height > MAX_DIMENSION
                || u64::from(clip.frame_width) * u64::from(clip.frame_height) > MAX_PIXELS {
                return Err(format!("{}: invalid frame dimensions in Protobuf header", frame_path.display()));
            }
            // Keep the same file-rate precedence as the upstream JSONL parser.
            let mut fps = None;
            for rate in [clip.file_frame_rate, clip.record_frame_rate, clip.sensor_frame_rate] {
                if rate == 0.0 { continue; }
                if !rate.is_finite() || rate < 0.0 || f64::from(rate) > MAX_FPS {
                    return Err(format!("{}: invalid frame rate in Protobuf header", frame_path.display()));
                }
                fps = Some(rate as f64);
                break;
            }
            let fps = fps.ok_or_else(|| format!("{}: missing frame rate in Protobuf header", frame_path.display()))?;
            let rotation = clip.rotation_degrees.rem_euclid(360);
            if rotation % 90 != 0 {
                return Err(format!("{}: unsupported rotation {} degrees", frame_path.display(), clip.rotation_degrees));
            }
            clip_properties = Some((clip.frame_width as usize, clip.frame_height as usize, fps, rotation));
            expected_header = Some(header.clone());
        } else if let Some(header) = &main.header {
            if Some(header) != expected_header.as_ref() {
                return Err(format!("{}: Protobuf header changed within the sequence", frame_path.display()));
            }
        }

        let frame = main.frame.as_ref()
            .ok_or_else(|| format!("{}: missing Protobuf frame metadata", frame_path.display()))?;
        let expected_number = u32::try_from(index + 1)
            .map_err(|_| "DNG sequence has too many frames".to_owned())?;
        if frame.frame_number != expected_number {
            return Err(format!("{}: Protobuf frame number {} does not match expected {}", frame_path.display(), frame.frame_number, expected_number));
        }

        let line = proto_json::to_jsonl_line(&main);
        if line.is_empty() {
            return Err(format!("{}: could not serialize Protobuf metadata", frame_path.display()));
        }
        let new_len = jsonl.len().checked_add(line.len()).and_then(|len| len.checked_add(1))
            .ok_or_else(|| "CDNG Protobuf JSONL stream is too large".to_owned())?;
        if new_len > MAX_JSONL_BYTES {
            return Err("CDNG Protobuf JSONL stream is too large".to_owned());
        }
        jsonl.extend_from_slice(line.as_bytes());
        jsonl.push(b'\n');
    }

    let (width, height, fps, rotation) = clip_properties.expect("nonempty sequence");
    Ok(SequenceData { jsonl, frame_count: paths.len(), width, height, fps, rotation, lens_missing })
}

/// Read every frame's carrier and turn the whole take into `Main` messages.
///
/// Serialized-protobuf carriers decode frame by frame; kind 2 records are
/// converted together, because samples are deduplicated and the accelerometer
/// is held across frame boundaries. A take must use one carrier throughout.
fn read_messages(paths: &[PathBuf], options: &fsg2::Options) -> Result<Vec<gyroflow_proto::Main>, String> {
    let mut protobuf = Vec::new();
    let mut gyro2 = Vec::new();
    for frame_path in paths {
        let in_frame = |error: String| format!("{}: {error}", frame_path.display());
        match read_frame(frame_path).map_err(in_frame)? {
            (FramePayload::Protobuf(bytes), _) => protobuf.push(
                gyroflow_proto::Main::decode(bytes.as_slice())
                    .map_err(|error| in_frame(format!("invalid Gyroflow Protobuf: {error}")))?,
            ),
            (FramePayload::Gyro2(bytes), dng) => gyro2.push((fsg2::parse(&bytes).map_err(in_frame)?, dng)),
        }
    }
    match (protobuf.is_empty(), gyro2.is_empty()) {
        (_, true) => Ok(protobuf),
        (true, false) => fsg2::to_messages_with(&gyro2, options),
        (false, false) => Err("DNG sequence mixes serialized-protobuf frames with kind 2 frames".to_owned()),
    }
}

pub(crate) fn is_dng_path(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dng"))
}

pub(crate) fn sequence_paths(first: &Path) -> Result<Vec<PathBuf>, String> {
    if !first.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dng")) {
        return Err(format!("{}: expected a DNG file", first.display()));
    }
    let stem = first.file_stem().and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("{}: DNG filename is not UTF-8", first.display()))?;
    let prefix_len = stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (prefix, digits) = stem.split_at(prefix_len);
    if digits.is_empty() {
        return Err(format!("{}: DNG filename needs a numeric frame suffix", first.display()));
    }
    let first_number: u64 = digits.parse()
        .map_err(|_| format!("{}: DNG frame number is too large", first.display()))?;
    let parent = first.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut numbered = BTreeMap::<u64, PathBuf>::new();
    for entry in fs::read_dir(parent).map_err(|error| format!("{}: {error}", parent.display()))? {
        let entry = entry.map_err(|error| format!("{}: {error}", parent.display()))?;
        let path = entry.path();
        if !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dng")) {
            continue;
        }
        let Some(other_stem) = path.file_stem().and_then(|name| name.to_str()) else { continue };
        let Some(other_digits) = other_stem.strip_prefix(prefix) else { continue };
        if other_digits.is_empty() || !other_digits.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let number: u64 = other_digits.parse()
            .map_err(|_| format!("{}: DNG frame number is too large", path.display()))?;
        if numbered.insert(number, path).is_some() {
            return Err(format!("duplicate DNG frame number {number} in {}", parent.display()));
        }
        if numbered.len() > MAX_FRAMES {
            return Err("DNG sequence has too many frames".to_owned());
        }
    }

    let first_path = numbered.get(&first_number)
        .ok_or_else(|| format!("{}: selected DNG frame does not exist", first.display()))?;
    if first_path.file_name() != first.file_name() {
        return Err(format!("{}: selected DNG frame does not exist", first.display()));
    }
    // The run of consecutive numbers that holds the selected frame: an editor
    // splits a numbered sequence at a gap (frames the camera never wrote), and
    // each part is a clip of its own. Each frame carries its own timing, so a
    // part loads like a whole take.
    let mut low = first_number;
    while low > 0 && numbered.contains_key(&(low - 1)) {
        low -= 1;
    }
    let mut paths = Vec::new();
    for (_, path) in numbered.range(low..).take_while({
        let mut expected = low;
        move |(number, _)| { let ok = **number == expected; expected = expected.wrapping_add(1); ok }
    }) {
        let path = path.clone();
        if !path.is_file() {
            return Err(format!("{}: DNG frame is not a regular file", path.display()));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn read_exact_at(file: &mut File, file_len: u64, offset: u64, out: &mut [u8]) -> Result<(), String> {
    let len = u64::try_from(out.len()).map_err(|_| "TIFF range is too large".to_owned())?;
    if offset.checked_add(len).is_none_or(|end| end > file_len) {
        return Err("TIFF offset or length exceeds file bounds".to_owned());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
    file.read_exact(out).map_err(|error| error.to_string())
}

/// One IFD entry as stored on disk; `value` holds the 4-byte value/offset field.
struct IfdEntry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

/// Minimal bounds-checked reader for the classic TIFF structures this module needs.
struct TiffReader {
    file: File,
    file_len: u64,
    order: ByteOrder,
}

impl TiffReader {
    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        let file_len = file.metadata().map_err(|error| error.to_string())?.len();
        let mut header = [0_u8; 8];
        read_exact_at(&mut file, file_len, 0, &mut header)?;
        let order = if &header[..2] == b"II" {
            ByteOrder::Little
        } else if &header[..2] == b"MM" {
            ByteOrder::Big
        } else {
            return Err("not a TIFF/DNG file".to_owned());
        };
        match order.u16(&header[2..4]) {
            42 => {}
            43 => return Err("BigTIFF is not supported".to_owned()),
            _ => return Err("not a classic TIFF/DNG file".to_owned()),
        }
        Ok(Self { file, file_len, order })
    }

    fn root_ifd_offset(&mut self) -> Result<u64, String> {
        let mut bytes = [0_u8; 4];
        read_exact_at(&mut self.file, self.file_len, 4, &mut bytes)?;
        Ok(u64::from(self.order.u32(&bytes)))
    }

    fn read_ifd(&mut self, ifd_offset: u64) -> Result<Vec<IfdEntry>, String> {
        let mut count_bytes = [0_u8; 2];
        read_exact_at(&mut self.file, self.file_len, ifd_offset, &mut count_bytes)?;
        let entry_count = u64::from(self.order.u16(&count_bytes));
        if entry_count > MAX_IFD_ENTRIES {
            return Err("TIFF IFD has too many entries".to_owned());
        }
        let entries_offset = ifd_offset.checked_add(2).ok_or("TIFF IFD offset overflow")?;
        let entries_len = entry_count.checked_mul(12).ok_or("TIFF IFD length overflow")?;
        if entries_offset.checked_add(entries_len).and_then(|end| end.checked_add(4)).is_none_or(|end| end > self.file_len) {
            return Err("TIFF IFD exceeds file bounds".to_owned());
        }
        let mut raw = vec![0_u8; entries_len as usize];
        read_exact_at(&mut self.file, self.file_len, entries_offset, &mut raw)?;
        Ok(raw.chunks_exact(12).map(|entry| IfdEntry {
            tag: self.order.u16(&entry[..2]),
            kind: self.order.u16(&entry[2..4]),
            count: self.order.u32(&entry[4..8]),
            value: entry[8..12].try_into().expect("four bytes"),
        }).collect())
    }

    /// Bytes of a BYTE/UNDEFINED entry, inline or out of line, at most `max` long.
    fn byte_data(&mut self, entry: &IfdEntry, max: usize, what: &str) -> Result<Vec<u8>, String> {
        if entry.kind != 1 && entry.kind != 7 {
            return Err(format!("{what} must be TIFF BYTE or UNDEFINED"));
        }
        let count = usize::try_from(entry.count)
            .map_err(|_| format!("{what} length is too large"))?;
        if count == 0 || count > max {
            return Err(format!("{what} length is invalid or too large"));
        }
        let mut data = vec![0_u8; count];
        if count <= 4 {
            data.copy_from_slice(&entry.value[..count]);
        } else {
            let offset = u64::from(self.order.u32(&entry.value));
            read_exact_at(&mut self.file, self.file_len, offset, &mut data)?;
        }
        Ok(data)
    }

    /// Raw bytes of any entry whose values total at most `MAX_VALUE_BYTES`.
    fn raw(&mut self, entry: &IfdEntry) -> Result<Vec<u8>, String> {
        let size = match entry.kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            kind => return Err(format!("unknown TIFF type {kind}")),
        };
        let len = (entry.count as usize).checked_mul(size).filter(|len| *len <= MAX_VALUE_BYTES)
            .ok_or("TIFF value is too large")?;
        let mut data = vec![0_u8; len];
        if len <= 4 {
            data.copy_from_slice(&entry.value[..len]);
        } else {
            let offset = u64::from(self.order.u32(&entry.value));
            read_exact_at(&mut self.file, self.file_len, offset, &mut data)?;
        }
        Ok(data)
    }

    fn ascii(&mut self, entry: &IfdEntry) -> Result<String, String> {
        if entry.kind != 2 { return Err("not ASCII".to_owned()); }
        let data = self.raw(entry)?;
        let end = data.iter().position(|byte| *byte == 0).unwrap_or(data.len());
        Ok(String::from_utf8_lossy(&data[..end]).into_owned())
    }

    /// Numeric values of an entry as f64; rationals are divided out.
    fn numbers(&mut self, entry: &IfdEntry) -> Result<Vec<f64>, String> {
        let data = self.raw(entry)?;
        let order = self.order;
        let n = entry.count as usize;
        let at = |i: usize, w: usize| &data[i * w..i * w + w];
        Ok(match entry.kind {
            1 => data.iter().map(|v| *v as f64).collect(),
            3 => (0..n).map(|i| order.u16(at(i, 2)) as f64).collect(),
            8 => (0..n).map(|i| order.u16(at(i, 2)) as i16 as f64).collect(),
            4 => (0..n).map(|i| order.u32(at(i, 4)) as f64).collect(),
            9 => (0..n).map(|i| order.u32(at(i, 4)) as i32 as f64).collect(),
            5 => (0..n).map(|i| {
                let (num, den) = (order.u32(&at(i, 8)[..4]), order.u32(&at(i, 8)[4..]));
                if den == 0 { f64::NAN } else { num as f64 / den as f64 }
            }).collect(),
            10 => (0..n).map(|i| {
                let (num, den) = (order.u32(&at(i, 8)[..4]) as i32, order.u32(&at(i, 8)[4..]) as i32);
                if den == 0 { f64::NAN } else { num as f64 / den as f64 }
            }).collect(),
            kind => return Err(format!("TIFF type {kind} is not numeric here")),
        })
    }

    /// Offset stored in a single LONG or IFD entry that points to a sub-IFD.
    fn sub_ifd_offset(&self, entry: &IfdEntry, what: &str) -> Result<u64, String> {
        if (entry.kind != 4 && entry.kind != 13) || entry.count != 1 {
            return Err(format!("{what} must be a single LONG or IFD offset"));
        }
        Ok(u64::from(self.order.u32(&entry.value)))
    }
}

fn find_unique<'a>(entries: &'a [IfdEntry], tag: u16, what: &str) -> Result<Option<&'a IfdEntry>, String> {
    let mut matches = entries.iter().filter(|entry| entry.tag == tag);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(format!("duplicate {what} in IFD"));
    }
    Ok(first)
}

/// What one DNG frame carries.
enum FramePayload {
    /// A serialized `gyroflow_proto::Main` (IFD0 tag 65000, or block kind 1).
    Protobuf(Vec<u8>),
    /// A SIGMA fp gyro record (block kind 2), decoded by `fsg2`.
    Gyro2(Vec<u8>),
}

#[cfg(test)]
fn read_frame_payload(path: &Path) -> Result<Vec<u8>, String> {
    read_frame(path).map(|(payload, _)| match payload {
        FramePayload::Protobuf(bytes) | FramePayload::Gyro2(bytes) => bytes,
    })
}

/// Read the telemetry carried by one DNG frame, and the frame's own metadata.
///
/// The carrier the fp writes since 2026-10-15 is a block in its own MakerNote
/// tag (see `parse_tag_block`); before that, a framed block in the unused zero
/// tail of the MakerNote (see `parse_makernote_block`), still read when the
/// tag is absent. The older prototype carrier, IFD0 tag 65000, is still
/// accepted and takes precedence when present.
fn read_frame(path: &Path) -> Result<(FramePayload, fsg2::DngInfo), String> {
    let mut tiff = TiffReader::open(path)?;
    let root_offset = tiff.root_ifd_offset()?;
    let root = tiff.read_ifd(root_offset)?;
    let exif = match find_unique(&root, EXIF_IFD_TAG, "Exif IFD pointer")? {
        Some(entry) => {
            let offset = tiff.sub_ifd_offset(entry, "Exif IFD pointer")?;
            tiff.read_ifd(offset)?
        }
        None => Vec::new(),
    };
    let dng = read_dng_info(&mut tiff, &root, &exif);

    if let Some(entry) = find_unique(&root, PROTOTYPE_TAG, "experimental Protobuf tag")? {
        let bytes = tiff.byte_data(entry, MAX_PROTO_BYTES, "experimental Protobuf tag payload")?;
        return Ok((FramePayload::Protobuf(bytes), dng));
    }
    if find_unique(&root, EXIF_IFD_TAG, "Exif IFD pointer")?.is_none() {
        return Err("no telemetry: IFD0 has neither tag 65000 nor an Exif IFD".to_owned());
    }
    let maker_note = find_unique(&exif, MAKER_NOTE_TAG, "MakerNote")?
        .ok_or_else(|| "no telemetry: Exif IFD has no MakerNote".to_owned())?;
    if let Some(tag) = gyro2_tag(&mut tiff, maker_note)? {
        let value = tiff.byte_data(&tag, MAX_PROTO_BYTES, "Gyro2 MakerNote tag")?;
        // The camera starts the block on the first word boundary of the file.
        let pad = (tiff.order.u32(&tag.value) as usize).wrapping_neg() % 4;
        let (kind, payload) = parse_tag_block(value.get(pad..).unwrap_or_default())?;
        let payload = payload.to_vec();
        return Ok((if kind == BLOCK_KIND_GYRO2 { FramePayload::Gyro2(payload) } else { FramePayload::Protobuf(payload) }, dng));
    }
    let maker_note = tiff.byte_data(maker_note, MAX_MAKER_NOTE_BYTES, "MakerNote")?;
    let (kind, payload) = parse_makernote_block(&maker_note)?;
    let payload = payload.to_vec();
    Ok((if kind == BLOCK_KIND_GYRO2 { FramePayload::Gyro2(payload) } else { FramePayload::Protobuf(payload) }, dng))
}

/// The frame's own EXIF/DNG values. Missing or malformed tags are left as
/// `None`: they are refinements, and kind 2 checks the ones it cannot do without.
fn read_dng_info(tiff: &mut TiffReader, root: &[IfdEntry], exif: &[IfdEntry]) -> fsg2::DngInfo {
    let mut text = |entries: &[IfdEntry], tag: u16| {
        entries.iter().find(|entry| entry.tag == tag).and_then(|entry| tiff.ascii(entry).ok())
            .map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
    };
    let make = text(root, 0x010F);
    let model = text(root, 0x0110);
    let software = text(root, 0x0131);
    let serial = text(root, 0xC62F);
    let lens_model = text(exif, 0xA434);
    let mut number = |entries: &[IfdEntry], tag: u16| -> Option<Vec<f64>> {
        entries.iter().find(|entry| entry.tag == tag).and_then(|entry| tiff.numbers(entry).ok())
    };
    let frame_size = number(root, 0xC620)
        .filter(|v| v.len() == 2 && v[0] > 0.0 && v[1] > 0.0 && v[0] <= MAX_DIMENSION as f64 && v[1] <= MAX_DIMENSION as f64)
        .map(|v| (v[0].round() as u32, v[1].round() as u32));
    let first = |v: Option<Vec<f64>>| v.and_then(|v| v.first().copied()).filter(|x| x.is_finite() && *x > 0.0);
    fsg2::DngInfo {
        make, model, serial, software, lens_model, frame_size,
        frame_rate: first(number(root, 0xC764)).filter(|fps| *fps <= MAX_FPS),
        exposure_s: first(number(exif, 0x829A)),
        iso: first(number(exif, 0x8827)).map(|iso| iso.round() as u32),
        f_number: first(number(exif, 0x829D)),
        focal_length_mm: first(number(exif, 0x920A)),
        subject_distance_m: first(number(exif, 0x9206)),
        warp_rectilinear: root.iter().find(|entry| entry.tag == OPCODE_LIST3_TAG)
            .and_then(|entry| tiff.raw(entry).ok())
            .and_then(|list| crate::lensfit::warp_rectilinear(&list)),
    }
}

/// Gyro2's own entry in a SIGMA MakerNote, if the MakerNote is SIGMA's and has one.
fn gyro2_tag(tiff: &mut TiffReader, maker_note: &IfdEntry) -> Result<Option<IfdEntry>, String> {
    if maker_note.count < 16 || (maker_note.kind != 7 && maker_note.kind != 1) {
        return Ok(None);
    }
    let start = u64::from(tiff.order.u32(&maker_note.value));
    let mut id = [0_u8; 8];
    if read_exact_at(&mut tiff.file, tiff.file_len, start, &mut id).is_err() || &id != SIGMA_MAKER_NOTE_ID {
        return Ok(None);
    }
    // A MakerNote this reader cannot walk is left to the tail search, which
    // never needed its IFD.
    let Ok(entries) = tiff.read_ifd(start + SIGMA_MAKER_NOTE_IFD_AT) else { return Ok(None) };
    Ok(find_unique(&entries, GYRO2_MAKER_NOTE_TAG, "Gyro2 MakerNote tag")?
        .map(|entry| IfdEntry { tag: entry.tag, kind: entry.kind, count: entry.count, value: entry.value }))
}

/// The block at the start of Gyro2's own MakerNote tag: the layout of
/// `parse_makernote_block`'s, version 2, and no CRC after it. Bytes 6..8,
/// reserved in version 1, hold the writing sup's build (its loader version),
/// for diagnostics only: the block and payload versions decide the layout. The tag is the
/// block's alone, so there is nothing to tell it from; the camera writes the
/// magic last and clears it when it has no block for the frame. The tag is
/// sized for the take; the bytes after the payload are left from earlier frames.
fn parse_tag_block(value: &[u8]) -> Result<(u8, &[u8]), String> {
    let header = value.get(..BLOCK_HEADER_BYTES).ok_or("Gyro2 MakerNote tag is too short for a block")?;
    if &header[..4] != MAKER_NOTE_MAGIC {
        return Err("no telemetry block in the Gyro2 MakerNote tag (the camera had none for this frame)".to_owned());
    }
    if header[4] != BLOCK_VERSION_2 {
        return Err(format!("unsupported block version {} in the Gyro2 MakerNote tag", header[4]));
    }
    let kind = header[5];
    if kind != BLOCK_KIND_GYRO2 {
        return Err(format!("unsupported block payload kind {kind}"));
    }
    let length = u32::from_le_bytes(header[8..12].try_into().expect("four bytes")) as usize;
    let payload = value.get(BLOCK_HEADER_BYTES..BLOCK_HEADER_BYTES.saturating_add(length))
        .filter(|_| length > 0)
        .ok_or("block payload length is invalid or exceeds the Gyro2 MakerNote tag")?;
    Ok((kind, payload))
}

/// Find the telemetry block inside a MakerNote and return its payload.
///
/// Block layout (all integers little-endian, independent of the TIFF byte
/// order, because the camera writes it as raw memory):
///
/// ```text
///   0  magic           4 bytes, MAKER_NOTE_MAGIC
///   4  version         u8, 1
///   5  payload kind    u8, 1 = serialized gyroflow_proto::Main,
///                          2 = SIGMA fp gyro record (see fsg2.rs)
///   6  reserved        u16, 0
///   8  payload length  u32
///  12  payload
///  12+len crc32        u32, IEEE CRC-32 of bytes 0 .. 12+len
/// ```
///
/// The block lives in the zero-filled tail the camera leaves at the end of
/// the declared MakerNote range, so it is carried along by anything that
/// copies the MakerNote, without adding IFD entries. Every candidate magic
/// must pass the length and CRC checks; exactly one valid block is required.
fn parse_makernote_block(maker_note: &[u8]) -> Result<(u8, &[u8]), String> {
    let mut found: Option<(u8, &[u8])> = None;
    let mut rejected = None;
    let mut start = 0;
    while let Some(position) = find_bytes(&maker_note[start..], MAKER_NOTE_MAGIC) {
        let at = start + position;
        start = at + 1;
        match parse_block_at(maker_note, at) {
            Ok(block) => {
                if found.is_some() {
                    return Err("more than one telemetry block in MakerNote".to_owned());
                }
                found = Some(block);
            }
            Err(error) => rejected = Some(error),
        }
    }
    match (found, rejected) {
        (Some(block), _) => Ok(block),
        (None, Some(error)) => Err(format!("invalid telemetry block in MakerNote: {error}")),
        (None, None) => Err("no telemetry block in MakerNote".to_owned()),
    }
}

fn parse_block_at(data: &[u8], at: usize) -> Result<(u8, &[u8]), String> {
    let header = data.get(at..at + BLOCK_HEADER_BYTES).ok_or("truncated block header")?;
    let version = header[4];
    let kind = header[5];
    if version != BLOCK_VERSION {
        return Err(format!("unsupported block version {version}"));
    }
    if header[6..8] != [0, 0] {
        return Err("reserved block bytes are not zero".to_owned());
    }
    let length = u32::from_le_bytes(header[8..12].try_into().expect("four bytes")) as usize;
    if length == 0 || length > MAX_PROTO_BYTES {
        return Err("block payload length is invalid or too large".to_owned());
    }
    let payload_end = at + BLOCK_HEADER_BYTES + length;
    let stored_crc = data.get(payload_end..payload_end + 4).ok_or("block exceeds MakerNote")?;
    let stored_crc = u32::from_le_bytes(stored_crc.try_into().expect("four bytes"));
    if crc32fast::hash(&data[at..payload_end]) != stored_crc {
        return Err("block CRC mismatch".to_owned());
    }
    if kind != BLOCK_KIND_PROTOBUF && kind != BLOCK_KIND_GYRO2 {
        return Err(format!("unsupported block payload kind {kind}"));
    }
    Ok((kind, &data[at + BLOCK_HEADER_BYTES..payload_end]))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("gyroflow-cdng-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn frame(&self, number: u32) -> PathBuf {
            self.0.join(format!("shot_{number:04}.dng"))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    fn message(number: u32, header: bool) -> gyroflow_proto::Main {
        let clip = gyroflow_proto::header::ClipMetadata {
            frame_width: 1920,
            frame_height: 1080,
            file_frame_rate: 23.976,
            rotation_degrees: -90,
            ..Default::default()
        };
        gyroflow_proto::Main {
            magic_string: "GyroflowProtobuf".to_owned(),
            protocol_version: 1,
            header: header.then(|| gyroflow_proto::Header {
                camera: Some(Default::default()),
                clip: Some(clip),
            }),
            frame: Some(gyroflow_proto::FrameMetadata { frame_number: number, ..Default::default() }),
        }
    }

    fn write_dng(path: &Path, main: &gyroflow_proto::Main, order: ByteOrder, kind: u16) {
        let payload = main.encode_to_vec();
        let mut bytes = Vec::new();
        let put_u16 = |out: &mut Vec<u8>, n: u16| match order {
            ByteOrder::Little => out.extend_from_slice(&n.to_le_bytes()),
            ByteOrder::Big => out.extend_from_slice(&n.to_be_bytes()),
        };
        let put_u32 = |out: &mut Vec<u8>, n: u32| match order {
            ByteOrder::Little => out.extend_from_slice(&n.to_le_bytes()),
            ByteOrder::Big => out.extend_from_slice(&n.to_be_bytes()),
        };
        bytes.extend_from_slice(match order { ByteOrder::Little => b"II", ByteOrder::Big => b"MM" });
        put_u16(&mut bytes, 42);
        put_u32(&mut bytes, 8);
        put_u16(&mut bytes, 1);
        put_u16(&mut bytes, PROTOTYPE_TAG);
        put_u16(&mut bytes, kind);
        put_u32(&mut bytes, payload.len() as u32);
        put_u32(&mut bytes, 26);
        put_u32(&mut bytes, 0);
        bytes.extend_from_slice(&payload);
        fs::write(path, bytes).unwrap();
    }

    fn block(payload: &[u8], kind: u8) -> Vec<u8> {
        let mut out = MAKER_NOTE_MAGIC.to_vec();
        out.extend_from_slice(&[BLOCK_VERSION, kind, 0, 0]);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        let crc = crc32fast::hash(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// A MakerNote shaped like the fp's: vendor data, then a zero tail that
    /// holds the given blocks with zero padding around each.
    fn maker_note_with(blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"SIGMA\0\0\0Ver.5.02".to_vec();
        out.extend((0..512_u32).map(|n| (n * 37 + 11) as u8));
        out.extend_from_slice(&[0; 300]);
        for block in blocks {
            out.extend_from_slice(block);
            out.extend_from_slice(&[0; 64]);
        }
        out.extend_from_slice(&[0; 200]);
        out
    }

    fn write_dng_maker_note(path: &Path, maker_note: &[u8], order: ByteOrder) {
        let mut bytes = Vec::new();
        let put_u16 = |out: &mut Vec<u8>, n: u16| match order {
            ByteOrder::Little => out.extend_from_slice(&n.to_le_bytes()),
            ByteOrder::Big => out.extend_from_slice(&n.to_be_bytes()),
        };
        let put_u32 = |out: &mut Vec<u8>, n: u32| match order {
            ByteOrder::Little => out.extend_from_slice(&n.to_le_bytes()),
            ByteOrder::Big => out.extend_from_slice(&n.to_be_bytes()),
        };
        bytes.extend_from_slice(match order { ByteOrder::Little => b"II", ByteOrder::Big => b"MM" });
        put_u16(&mut bytes, 42);
        put_u32(&mut bytes, 8);
        // IFD0 at 8: one Exif IFD pointer.
        put_u16(&mut bytes, 1);
        put_u16(&mut bytes, EXIF_IFD_TAG);
        put_u16(&mut bytes, 4);
        put_u32(&mut bytes, 1);
        put_u32(&mut bytes, 26);
        put_u32(&mut bytes, 0);
        // Exif IFD at 26: one MakerNote entry whose data starts at 44.
        put_u16(&mut bytes, 1);
        put_u16(&mut bytes, MAKER_NOTE_TAG);
        put_u16(&mut bytes, 7);
        put_u32(&mut bytes, maker_note.len() as u32);
        put_u32(&mut bytes, 44);
        put_u32(&mut bytes, 0);
        bytes.extend_from_slice(maker_note);
        fs::write(path, bytes).unwrap();
    }

    fn write_maker_note_frame(path: &Path, main: &gyroflow_proto::Main, order: ByteOrder) {
        let note = maker_note_with(&[block(&main.encode_to_vec(), BLOCK_KIND_PROTOBUF)]);
        write_dng_maker_note(path, &note, order);
    }

    #[test]
    fn reads_little_and_big_endian_sequence_as_jsonl() {
        let dir = TestDir::new();
        write_dng(&dir.frame(1), &message(1, true), ByteOrder::Little, 1);
        write_dng(&dir.frame(2), &message(2, false), ByteOrder::Big, 7);
        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        assert_eq!(sequence.frame_count, 2);
        assert_eq!((sequence.width, sequence.height, sequence.rotation), (1920, 1080, 270));
        assert!((sequence.fps - 23.976).abs() < 0.001);
        let lines: Vec<_> = sequence.jsonl.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).collect();
        assert_eq!(lines.len(), 2);
        assert!(std::str::from_utf8(lines[0]).unwrap().contains("GyroflowProtobuf"));
        assert!(std::str::from_utf8(lines[1]).unwrap().contains("frameNumber"));

        let from_second = read_embedded_protobuf_sequence(&dir.frame(2)).unwrap();
        assert_eq!(from_second.frame_count, 2);
        assert_eq!(from_second.jsonl, sequence.jsonl);
    }

    #[test]
    fn core_consumes_two_frame_sequence_as_jsonl() {
        let dir = TestDir::new();
        let mut first = message(1, true);
        first.frame.as_mut().unwrap().start_timestamp_us = 1_000_000.0;
        first.frame.as_mut().unwrap().end_timestamp_us = 1_005_000.0;
        let mut second = message(2, false);
        second.frame.as_mut().unwrap().start_timestamp_us = 1_041_708.0;
        second.frame.as_mut().unwrap().end_timestamp_us = 1_046_708.0;
        write_dng(&dir.frame(1), &first, ByteOrder::Little, 7);
        write_dng(&dir.frame(2), &second, ByteOrder::Little, 7);

        let sequence = read_embedded_protobuf_sequence(&dir.frame(2)).unwrap();
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let virtual_path = dir.0.join("embedded-telemetry.jsonl");
        let virtual_url = gyroflow_core::filesystem::path_to_url(&virtual_path.to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &virtual_url, Some(metadata), true).unwrap();
        assert_eq!(manager.gyro.read().file_metadata.read().per_frame_time_offsets.len(), 2);
    }

    #[test]
    fn a_gap_in_dng_numbering_splits_the_take_into_parts() {
        let dir = TestDir::new();
        let per_frame = 83;
        for (file, seq) in [(1, 0), (2, 1), (3, 2), (7, 6), (8, 7)] {      // files 4-6 never written
            let record = gyro2_record(seq * per_frame, seq, per_frame as usize + 4);
            write_fp_like_frame(&dir.frame(file), block(&fsg2::encode(&record), BLOCK_KIND_GYRO2));
        }
        assert_eq!(read_embedded_protobuf_sequence(&dir.frame(2)).unwrap().frame_count, 3);
        assert_eq!(read_embedded_protobuf_sequence(&dir.frame(7)).unwrap().frame_count, 2);
    }

    #[test]
    fn accepts_numeric_suffix_that_grows_in_width() {
        let dir = TestDir::new();
        let first = dir.frame(9999);
        let second = dir.frame(10000);
        write_dng(&first, &message(1, true), ByteOrder::Little, 7);
        write_dng(&second, &message(2, false), ByteOrder::Little, 7);
        assert_eq!(read_embedded_protobuf_sequence(&second).unwrap().frame_count, 2);
    }

    #[test]
    fn rejects_protobuf_frame_gap() {
        let dir = TestDir::new();
        write_dng(&dir.frame(1), &message(1, true), ByteOrder::Little, 7);
        write_dng(&dir.frame(2), &message(3, false), ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&dir.frame(1)).unwrap_err().contains("frame number 3"));
    }

    #[test]
    fn rejects_out_of_bounds_payload_without_reading_pixels() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        write_dng(&path, &message(1, true), ByteOrder::Little, 7);
        let mut bytes = fs::read(&path).unwrap();
        bytes[18..22].copy_from_slice(&(MAX_PROTO_BYTES as u32).to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("exceeds file bounds"));
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        let mut main = message(1, true);
        main.magic_string = "other".to_owned();
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("invalid Gyroflow Protobuf magic"));
        main.magic_string = "GyroflowProtobuf".to_owned();
        main.protocol_version = 2;
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("unsupported Gyroflow Protobuf version"));
    }

    #[test]
    fn rejects_missing_clip_and_invalid_rate() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        let mut main = message(1, true);
        main.header.as_mut().unwrap().clip = None;
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("no clip metadata"));

        main.header.as_mut().unwrap().clip = Some(gyroflow_proto::header::ClipMetadata {
            frame_width: 1920,
            frame_height: 1080,
            file_frame_rate: f32::NAN,
            record_frame_rate: 24.0,
            ..Default::default()
        });
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("invalid frame rate"));
    }

    #[test]
    fn rejects_implausible_clip_properties() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        let mut main = message(1, true);
        let clip = main.header.as_mut().unwrap().clip.as_mut().unwrap();
        clip.frame_width = MAX_DIMENSION + 1;
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("invalid frame dimensions"));

        let clip = main.header.as_mut().unwrap().clip.as_mut().unwrap();
        clip.frame_width = 12_000;
        clip.frame_height = 12_000;
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("invalid frame dimensions"));

        let clip = main.header.as_mut().unwrap().clip.as_mut().unwrap();
        clip.frame_width = 1920;
        clip.frame_height = 1080;
        clip.file_frame_rate = 1001.0;
        write_dng(&path, &main, ByteOrder::Little, 7);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("invalid frame rate"));
    }

    #[test]
    fn rejects_wrong_tiff_tag_type() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        write_dng(&path, &message(1, true), ByteOrder::Big, 2);
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("BYTE or UNDEFINED"));
    }

    #[test]
    fn rejects_big_tiff() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        fs::write(&path, b"II\x2b\0\x08\0\0\0").unwrap();
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("BigTIFF"));
    }

    #[test]
    fn rejects_excessive_ifd_entry_count() {
        let dir = TestDir::new();
        let path = dir.frame(1);
        write_dng(&path, &message(1, true), ByteOrder::Little, 7);
        let mut bytes = fs::read(&path).unwrap();
        bytes[8..10].copy_from_slice(&((MAX_IFD_ENTRIES + 1) as u16).to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(read_embedded_protobuf_sequence(&path).unwrap_err().contains("too many entries"));
    }

    #[test]
    fn reads_sequence_from_maker_note_tail_in_either_tiff_order() {
        let dir = TestDir::new();
        write_maker_note_frame(&dir.frame(1), &message(1, true), ByteOrder::Little);
        write_maker_note_frame(&dir.frame(2), &message(2, false), ByteOrder::Big);
        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        assert_eq!(sequence.frame_count, 2);
        assert_eq!((sequence.width, sequence.height), (1920, 1080));
        let lines = sequence.jsonl.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).count();
        assert_eq!(lines, 2);
    }

    #[test]
    fn maker_note_and_tag_carriers_give_identical_streams() {
        let tag_dir = TestDir::new();
        let note_dir = TestDir::new();
        for (number, header) in [(1, true), (2, false)] {
            write_dng(&tag_dir.frame(number), &message(number, header), ByteOrder::Little, 7);
            write_maker_note_frame(&note_dir.frame(number), &message(number, header), ByteOrder::Little);
        }
        let from_tag = read_embedded_protobuf_sequence(&tag_dir.frame(1)).unwrap();
        let from_note = read_embedded_protobuf_sequence(&note_dir.frame(1)).unwrap();
        assert_eq!(from_tag.jsonl, from_note.jsonl);
    }

    #[test]
    fn core_consumes_maker_note_sequence() {
        let dir = TestDir::new();
        let mut first = message(1, true);
        first.frame.as_mut().unwrap().start_timestamp_us = 1_000_000.0;
        first.frame.as_mut().unwrap().end_timestamp_us = 1_005_000.0;
        let mut second = message(2, false);
        second.frame.as_mut().unwrap().start_timestamp_us = 1_041_708.0;
        second.frame.as_mut().unwrap().end_timestamp_us = 1_046_708.0;
        write_maker_note_frame(&dir.frame(1), &first, ByteOrder::Little);
        write_maker_note_frame(&dir.frame(2), &second, ByteOrder::Little);

        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let virtual_path = dir.0.join("embedded-telemetry.jsonl");
        let virtual_url = gyroflow_core::filesystem::path_to_url(&virtual_path.to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &virtual_url, Some(metadata), true).unwrap();
        assert_eq!(manager.gyro.read().file_metadata.read().per_frame_time_offsets.len(), 2);
    }

    #[test]
    fn maker_note_block_crc_mismatch_is_rejected() {
        let mut bad = block(&message(1, true).encode_to_vec(), BLOCK_KIND_PROTOBUF);
        let last_payload_byte = bad.len() - 5;
        bad[last_payload_byte] ^= 0xFF;
        let error = parse_makernote_block(&maker_note_with(&[bad])).unwrap_err();
        assert!(error.contains("CRC mismatch"), "{error}");
    }

    #[test]
    fn stray_magic_in_vendor_data_does_not_hide_the_real_block() {
        let real = block(&message(1, true).encode_to_vec(), BLOCK_KIND_PROTOBUF);
        let mut note = b"vendor FSG2 text that is not a block ".to_vec();
        note.extend(maker_note_with(&[real.clone()]));
        assert_eq!(parse_makernote_block(&note).unwrap(), (BLOCK_KIND_PROTOBUF, &real[BLOCK_HEADER_BYTES..real.len() - 4]));
    }

    #[test]
    fn two_valid_maker_note_blocks_are_ambiguous() {
        let one = block(&message(1, true).encode_to_vec(), BLOCK_KIND_PROTOBUF);
        let two = block(&message(2, false).encode_to_vec(), BLOCK_KIND_PROTOBUF);
        let error = parse_makernote_block(&maker_note_with(&[one, two])).unwrap_err();
        assert!(error.contains("more than one"), "{error}");
    }

    #[test]
    fn maker_note_without_block_and_unknown_kind_are_reported() {
        let error = parse_makernote_block(&maker_note_with(&[])).unwrap_err();
        assert!(error.contains("no telemetry block"), "{error}");
        let unknown = block(b"a payload kind nobody defined", 3);
        let error = parse_makernote_block(&maker_note_with(&[unknown])).unwrap_err();
        assert!(error.contains("unsupported block payload kind 3"), "{error}");
    }

    #[test]
    fn block_running_past_maker_note_end_is_rejected() {
        let mut truncated = block(&message(1, true).encode_to_vec(), BLOCK_KIND_PROTOBUF);
        truncated.truncate(truncated.len() - 2);
        let error = parse_makernote_block(&truncated).unwrap_err();
        assert!(error.contains("exceeds MakerNote"), "{error}");
    }

    #[test]
    fn frame_without_any_carrier_explains_what_is_missing() {
        let dir = TestDir::new();
        write_dng_maker_note(&dir.frame(1), &maker_note_with(&[]), ByteOrder::Little);
        let error = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap_err();
        assert!(error.contains("no telemetry block in MakerNote"), "{error}");
    }

    /// Real-camera check, skipped by default because it needs a SIGMA fp DNG:
    /// `FP_DNG_SAMPLE=/path/A001_001_..._000001.DNG cargo test real_fp -- --ignored`
    /// Writes a block into the zero tail of the real MakerNote, the way the
    /// camera will, and reads it back through the plugin's reader.
    #[test]
    #[ignore]
    fn real_fp_dng_maker_note_tail_round_trip() {
        let source = std::env::var("FP_DNG_SAMPLE").expect("set FP_DNG_SAMPLE to an fp CinemaDNG frame");
        let original = fs::read(&source).unwrap();
        let mut tiff = TiffReader::open(Path::new(&source)).unwrap();
        let root_offset = tiff.root_ifd_offset().unwrap();
        let root = tiff.read_ifd(root_offset).unwrap();
        let exif_offset = tiff.sub_ifd_offset(find_unique(&root, EXIF_IFD_TAG, "Exif").unwrap().unwrap(), "Exif").unwrap();
        let exif = tiff.read_ifd(exif_offset).unwrap();
        let note = find_unique(&exif, MAKER_NOTE_TAG, "MakerNote").unwrap().unwrap();
        let note_start = tiff.order.u32(&note.value) as usize;
        let note_end = note_start + note.count as usize;
        let tail_start = note_end - original[note_start..note_end].iter().rev().take_while(|byte| **byte == 0).count();
        let tail = note_end - tail_start;
        println!("MakerNote {note_start:#x}..{note_end:#x}, zero tail {tail_start:#x}.. ({tail} bytes)");

        let dir = TestDir::new();
        for number in [1_u32, 2] {
            let encoded = block(&message(number, number == 1).encode_to_vec(), BLOCK_KIND_PROTOBUF);
            // Leave a gap after the vendor data, as the camera writer should.
            let at = tail_start + 64;
            assert!(at + encoded.len() <= note_end, "block does not fit in the zero tail");
            let mut frame = original.clone();
            frame[at..at + encoded.len()].copy_from_slice(&encoded);
            fs::write(dir.frame(number), frame).unwrap();
        }
        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        assert_eq!(sequence.frame_count, 2);
        // The untouched camera frame has no block: only the injected copies do.
        assert!(read_frame_payload(Path::new(&source)).unwrap_err().contains("no telemetry block"));
    }

    /// A little-endian TIFF with the given IFD0 and Exif IFD entries
    /// (tag, type, count, value bytes); the Exif pointer is added and every
    /// IFD is sorted, as TIFF requires.
    fn tiff_le(mut root: Vec<(u16, u16, u32, Vec<u8>)>, mut exif: Vec<(u16, u16, u32, Vec<u8>)>) -> Vec<u8> {
        root.push((EXIF_IFD_TAG, 4, 1, vec![0; 4]));
        root.sort_by_key(|entry| entry.0);
        exif.sort_by_key(|entry| entry.0);
        let ifd_len = |n: usize| 2 + 12 * n + 4;
        let root_at = 8;
        let exif_at = root_at + ifd_len(root.len());
        let mut data_at = exif_at + ifd_len(exif.len());
        let mut out = b"II\x2a\0".to_vec();
        out.extend_from_slice(&(root_at as u32).to_le_bytes());
        let mut data = Vec::new();
        let mut write_ifd = |out: &mut Vec<u8>, entries: &[(u16, u16, u32, Vec<u8>)], data_at: &mut usize| {
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (tag, kind, count, bytes) in entries {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&kind.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                if *tag == EXIF_IFD_TAG {
                    out.extend_from_slice(&(exif_at as u32).to_le_bytes());
                } else if bytes.len() <= 4 {
                    let mut inline = bytes.clone();
                    inline.resize(4, 0);
                    out.extend_from_slice(&inline);
                } else {
                    out.extend_from_slice(&(*data_at as u32).to_le_bytes());
                    *data_at += bytes.len();
                    data.extend_from_slice(bytes);
                }
            }
            out.extend_from_slice(&0_u32.to_le_bytes());
        };
        write_ifd(&mut out, &root, &mut data_at);
        write_ifd(&mut out, &exif, &mut data_at);
        out.extend_from_slice(&data);
        out
    }

    fn ascii(text: &str) -> (u32, Vec<u8>) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        (bytes.len() as u32, bytes)
    }

    fn rational(num: u32, den: u32) -> Vec<u8> {
        [num.to_le_bytes(), den.to_le_bytes()].concat()
    }

    /// An fp-like CinemaDNG frame: FHD, 29.97 fps, 1/60 s, with `block` in the MakerNote tail.
    fn write_fp_like_frame(path: &Path, block: Vec<u8>) {
        write_fp_like_frame_with(path, maker_note_with(&[block]), false);
    }

    fn block_v2(payload: &[u8]) -> Vec<u8> {
        let mut out = MAKER_NOTE_MAGIC.to_vec();
        out.extend_from_slice(&[BLOCK_VERSION_2, BLOCK_KIND_GYRO2, 5, 0]);      // written by build 5
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A SIGMA MakerNote whose IFD holds a tag 0x0200 and Gyro2's tag of
    /// `cap` bytes with `value` at its start; the rest of the tag is what
    /// earlier frames left. Offsets count from the MakerNote until
    /// `write_fp_like_frame_with` moves them to the file's.
    fn sigma_maker_note(value: &[u8], cap: usize) -> Vec<u8> {
        let mut out = SIGMA_MAKER_NOTE_ID.to_vec();
        out.extend_from_slice(&[1, 4]);
        out.extend_from_slice(&2_u16.to_le_bytes());
        for (tag, count, field) in [(0x0200_u16, 4_u32, *b"side"), (GYRO2_MAKER_NOTE_TAG, cap as u32, 40_u32.to_le_bytes())] {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&7_u16.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&field);
        }
        out.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(out.len(), 40);
        let mut tag = value.to_vec();
        tag.resize(cap, 0x5A);
        out.extend_from_slice(&tag);
        out
    }

    fn write_fp_like_frame_with(path: &Path, note: Vec<u8>, sigma_offsets: bool) {
        let (make_n, make) = ascii("SIGMA");
        let (model_n, model) = ascii("SIGMA fp");
        let (lens_n, lens) = ascii("28mm F1.4 DG HSM | Art 019");
        let bytes = tiff_le(
            vec![
                (0x010F, 2, make_n, make),
                (0x0110, 2, model_n, model),
                (0xC620, 3, 2, [1920_u16.to_le_bytes(), 1080_u16.to_le_bytes()].concat()),
                (0xC764, 10, 1, rational(30000, 1001)),
            ],
            vec![
                (0x829A, 5, 1, rational(1, 60)),
                (0x8827, 3, 1, 100_u16.to_le_bytes().to_vec()),
                (0x920A, 5, 1, rational(28, 1)),
                (0x9206, 5, 1, rational(403, 1000)),
                (0xA434, 2, lens_n, lens),
                (MAKER_NOTE_TAG, 7, note.len() as u32, note.clone()),
            ],
        );
        let mut bytes = bytes;
        if sigma_offsets {
            let start = find_bytes(&bytes, &note).unwrap();
            let n = u16::from_le_bytes([bytes[start + 10], bytes[start + 11]]) as usize;
            for k in 0..n {
                let entry = start + 12 + 12 * k;
                let count = u32::from_le_bytes(bytes[entry + 4..entry + 8].try_into().unwrap());
                if count > 4 {
                    let at = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap()) + start as u32;
                    bytes[entry + 8..entry + 12].copy_from_slice(&at.to_le_bytes());
                    // Gyro2's block starts on the file's next word boundary, as the camera puts it.
                    let tag = u16::from_le_bytes([bytes[entry], bytes[entry + 1]]);
                    let (at, count) = (at as usize, count as usize);
                    let pad = at.wrapping_neg() % 4;
                    if tag == GYRO2_MAKER_NOTE_TAG && pad != 0 {
                        bytes[at..at + count].rotate_right(pad);
                        bytes[at..at + pad].fill(0);
                    }
                }
            }
        }
        fs::write(path, bytes).unwrap();
    }

    fn gyro2_record(base_idx: u32, frame_seq: u32, n: usize) -> fsg2::Record {
        fsg2::Record {
            fmt_version: fsg2::FMT_VERSION,
            orientation: 0,
            flags: if frame_seq == 0 { fsg2::FLAG_FIRST_FRAME } else { 0 },
            clip_id: 0xC0FFEE,
            frame_seq,
            sample_period_ps: 400_085_400,
            gscale: 0.000137923,
            ascale: 0.0009765625,
            readout_ns: 10_556_000,
            mode_id: 106,
            crop: [0, 0, 6000, 3376],
            sensor_mode: None,
            calib_focal_tenths_mm: None,
            frame_mark: 4,
            base_idx,
            // A slow pan: about 5 deg/s on the first axis, with some wobble.
            samples: (0..n).map(|i| [600 + (i as i16 % 7) * 3, -20, 15]).collect(),
            events: vec![
                fsg2::Event { pos: 0, kind: fsg2::EVENT_VD, value: (frame_seq * 33_367).to_le_bytes().to_vec() },
                fsg2::Event { pos: 1, kind: fsg2::EVENT_LEVEL, value: [12_i16, -8, 1020].iter().flat_map(|v| v.to_le_bytes()).collect() },
            ],
            lens_table: None,
        }
    }

    #[test]
    fn kind2_sequence_runs_through_core() {
        let dir = TestDir::new();
        // 29.97 fps at 2499.5 Hz is about 83.4 samples a frame; each block
        // carries 4 guard samples that overlap the next one.
        let per_frame = 83;
        let frames = 5_u32;
        for seq in 0..frames {
            let mut record = gyro2_record(seq * per_frame, seq, per_frame as usize + 4);
            if seq == 0 {
                record.flags |= fsg2::FLAG_LENS_TABLE;
                record.lens_table = Some(fsg2::LensTable {
                    calib_focal_tenths_mm: 280,
                    axis: [0, 18641, 37283, 49637, 55924],
                    nodes: [[[1.0003, -0.0148, 0.0052, -0.0098]; 3]; 5],
                });
            }
            write_fp_like_frame(&dir.frame(seq + 1), block(&fsg2::encode(&record), BLOCK_KIND_GYRO2));
        }
        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        assert_eq!(sequence.frame_count, frames as usize);
        assert_eq!((sequence.width, sequence.height), (1920, 1080));
        assert!((sequence.fps - 29.97).abs() < 0.01);

        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let virtual_url = gyroflow_core::filesystem::path_to_url(&dir.0.join("t.jsonl").to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &virtual_url, Some(metadata), true).unwrap();
        let gyro = manager.gyro.read();
        let file_metadata = gyro.file_metadata.read();
        assert_eq!(file_metadata.per_frame_time_offsets.len(), frames as usize);
        // Guards are deduplicated: every global sample index appears once.
        assert_eq!(file_metadata.raw_imu.len(), (frames * per_frame + 4) as usize);
        assert!(file_metadata.has_accurate_timestamps);
        assert_eq!(file_metadata.imu_orientation.as_deref(), Some("xyz"));
        let readout_ms = file_metadata.frame_readout_time.unwrap();
        assert!((readout_ms - 10.556).abs() < 1e-3, "{readout_ms}");
        assert!(!gyro.quaternions.is_empty());
        assert!(gyro.quaternions.values().all(|q| q.coords.iter().all(|c| c.is_finite())));
        drop(file_metadata);
        drop(gyro);
        check_lens_reaches_core(&manager, frames as usize);
    }

    fn gyro2_record_v2(base_idx: u32, frame_seq: u32, n: usize) -> fsg2::Record {
        let mode = fsg2::SensorMode { hmax: 380, read_w: 3000, lines: 2000, bin_h: 2, bin_v: 2 };
        fsg2::Record {
            fmt_version: fsg2::FMT_VERSION_2,
            readout_ns: mode.readout_ns(),
            crop: mode.crop(),
            sensor_mode: Some(mode),
            calib_focal_tenths_mm: Some(280),
            ..gyro2_record(base_idx, frame_seq, n)
        }
    }

    #[test]
    fn version_2_in_its_own_tag_runs_through_core() {
        let dir = TestDir::new();
        // The first block carries the oldest 120 of 250 + 83 samples; the
        // next ones catch up 120 at a time.
        let frames = 5_u32;
        let marks: Vec<u32> = (0..frames).map(|k| 333 + 83 * k).collect();
        let mut at = 0;
        for seq in 0..frames {
            let end = marks[seq as usize].min(at + 120);
            let mut record = gyro2_record_v2(at, seq, (end - at) as usize);
            record.frame_mark = (marks[seq as usize] - at) as u16;
            // Each Vd (35 samples before its hook) goes in the block that spans it.
            record.events.retain(|event| event.kind != fsg2::EVENT_VD);
            for vd in marks.iter().map(|mark| mark - 35).filter(|vd| (at..end).contains(vd)) {
                record.events.push(fsg2::Event { pos: (vd - at) as u16, kind: fsg2::EVENT_VD, value: vec![0; 4] });
            }
            let encoded = block_v2(&fsg2::encode(&record));
            write_fp_like_frame_with(&dir.frame(seq + 1), sigma_maker_note(&encoded, 12 + 48 + 6 * 120 + 288), true);
            at = end;
        }
        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        assert_eq!(sequence.frame_count, frames as usize);
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let virtual_url = gyroflow_core::filesystem::path_to_url(&dir.0.join("t.jsonl").to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &virtual_url, Some(metadata), true).unwrap();
        let gyro = manager.gyro.read();
        let file_metadata = gyro.file_metadata.read();
        assert_eq!(file_metadata.raw_imu.len(), at as usize);
        let readout_ms = file_metadata.frame_readout_time.unwrap();
        assert!((readout_ms - 10.556).abs() < 1e-3, "{readout_ms}");
        drop(file_metadata);
        drop(gyro);
        // No lens table: the frames have no WarpRectilinear either, so no fit.
        let first = fs::read(dir.frame(1)).unwrap();
        assert!(find_bytes(&first, b"FSG2").is_some());
    }

    #[test]
    fn the_gyro2_tag_is_read_before_any_tail_block() {
        let dir = TestDir::new();
        let record = gyro2_record_v2(0, 0, 10);
        write_fp_like_frame_with(&dir.frame(1), sigma_maker_note(&block_v2(&fsg2::encode(&record)), 400), true);
        match read_frame(&dir.frame(1)).unwrap().0 {
            FramePayload::Gyro2(payload) => assert_eq!(fsg2::parse(&payload).unwrap(), record),
            FramePayload::Protobuf(_) => panic!("kind 2 expected"),
        }
    }

    #[test]
    fn the_block_starts_on_the_first_word_boundary_of_the_file() {
        let dir = TestDir::new();
        let record = gyro2_record_v2(0, 0, 10);
        write_fp_like_frame_with(&dir.frame(1), sigma_maker_note(&block_v2(&fsg2::encode(&record)), 400), true);
        let bytes = fs::read(dir.frame(1)).unwrap();
        let note = find_bytes(&bytes, SIGMA_MAKER_NOTE_ID).unwrap();
        let value = note + 40;
        assert_ne!(value % 4, 0, "the test needs a value that is not word aligned");
        assert_eq!(find_bytes(&bytes, b"FSG2").unwrap(), (value + 3) & !3);
        assert!(matches!(read_frame(&dir.frame(1)).unwrap().0, FramePayload::Gyro2(_)));
    }

    #[test]
    fn a_gyro2_tag_without_a_block_or_with_a_bad_one_is_reported() {
        let dir = TestDir::new();
        write_fp_like_frame_with(&dir.frame(1), sigma_maker_note(&[0; 16], 400), true);
        assert!(read_frame(&dir.frame(1)).err().unwrap().contains("had none"));
        let mut long = block_v2(&fsg2::encode(&gyro2_record_v2(0, 0, 10)));
        long[8..12].copy_from_slice(&1000_u32.to_le_bytes());
        write_fp_like_frame_with(&dir.frame(2), sigma_maker_note(&long, 400), true);
        assert!(read_frame(&dir.frame(2)).err().unwrap().contains("exceeds"));
        let mut v1 = block_v2(&fsg2::encode(&gyro2_record_v2(0, 0, 10)));
        v1[4] = BLOCK_VERSION;
        write_fp_like_frame_with(&dir.frame(3), sigma_maker_note(&v1, 400), true);
        assert!(read_frame(&dir.frame(3)).err().unwrap().contains("version 1"));
    }

    /// Each frame's fitted lens lands in gyroflow-core's per-frame lens
    /// parameters, and a lens profile is synthesized for the take.
    fn check_lens_reaches_core(manager: &gyroflow_core::StabilizationManager, frames: usize) {
        let gyro = manager.gyro.read();
        let file_metadata = gyro.file_metadata.read();
        assert!(file_metadata.lens_profile.as_ref().is_some_and(|p| p["distortion_model"] == "opencv_fisheye"), "{:?}", file_metadata.lens_profile);
        assert_eq!(file_metadata.lens_params.len(), frames);
        let first = file_metadata.lens_params.values().next().unwrap();
        assert_eq!(first.distortion_coefficients.len(), 4);
        assert!(first.pixel_focal_length.is_some_and(|(fx, fy)| fx > 0.0 && fx == fy));
    }

    #[test]
    fn kind2_and_protobuf_frames_cannot_share_a_take() {
        let dir = TestDir::new();
        write_maker_note_frame(&dir.frame(1), &message(1, true), ByteOrder::Little);
        write_fp_like_frame(&dir.frame(2), block(&fsg2::encode(&gyro2_record(0, 1, 10)), BLOCK_KIND_GYRO2));
        let error = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap_err();
        assert!(error.contains("mixes"), "{error}");
    }

    #[test]
    fn dng_info_reads_fp_like_tags() {
        let dir = TestDir::new();
        write_fp_like_frame(&dir.frame(1), block(&fsg2::encode(&gyro2_record(0, 0, 1)), BLOCK_KIND_GYRO2));
        let (_, dng) = read_frame(&dir.frame(1)).unwrap();
        assert_eq!(dng.make.as_deref(), Some("SIGMA"));
        assert_eq!(dng.lens_model.as_deref(), Some("28mm F1.4 DG HSM | Art 019"));
        assert_eq!(dng.frame_size, Some((1920, 1080)));
        assert!((dng.frame_rate.unwrap() - 29.97).abs() < 0.01);
        assert!((dng.exposure_s.unwrap() - 1.0 / 60.0).abs() < 1e-9);
        assert_eq!(dng.iso, Some(100));
        assert_eq!(dng.focal_length_mm, Some(28.0));
        assert!((dng.subject_distance_m.unwrap() - 0.403).abs() < 1e-9);
    }

    /// Real-camera check for kind 2, skipped by default:
    /// `FP_DNG_SAMPLE=/path/A001_001_..._000001.DNG cargo test real_fp -- --ignored --nocapture`
    /// Writes kind 2 blocks into the MakerNote tail of copies of a real frame,
    /// so the real EXIF (size, rate, exposure, lens) feeds the conversion, and
    /// loads the result into gyroflow-core.
    #[test]
    #[ignore]
    fn real_fp_dng_kind2_through_core() {
        let source = std::env::var("FP_DNG_SAMPLE").expect("set FP_DNG_SAMPLE to an fp CinemaDNG frame");
        let original = fs::read(&source).unwrap();
        let mut tiff = TiffReader::open(Path::new(&source)).unwrap();
        let root_offset = tiff.root_ifd_offset().unwrap();
        let root = tiff.read_ifd(root_offset).unwrap();
        let exif_offset = tiff.sub_ifd_offset(find_unique(&root, EXIF_IFD_TAG, "Exif").unwrap().unwrap(), "Exif").unwrap();
        let exif = tiff.read_ifd(exif_offset).unwrap();
        let note = find_unique(&exif, MAKER_NOTE_TAG, "MakerNote").unwrap().unwrap();
        let note_start = tiff.order.u32(&note.value) as usize;
        let note_end = note_start + note.count as usize;
        let tail_start = note_end - original[note_start..note_end].iter().rev().take_while(|byte| **byte == 0).count();

        let dir = TestDir::new();
        let per_frame = 83_u32;
        let frames = 4_u32;
        let mut largest = 0;
        for seq in 0..frames {
            let mut record = gyro2_record(seq * per_frame, seq, per_frame as usize + 4);
            if seq == 0 {
                record.flags |= fsg2::FLAG_LENS_TABLE;
                record.lens_table = Some(fsg2::LensTable { calib_focal_tenths_mm: 280, axis: [0, 18641, 37283, 49637, 55924], nodes: [[[1.0, -0.0148, 0.0052, -0.0098]; 3]; 5] });
            }
            let encoded = block(&fsg2::encode(&record), BLOCK_KIND_GYRO2);
            largest = largest.max(encoded.len());
            let at = tail_start + 64;
            assert!(at + encoded.len() <= note_end, "block of {} bytes does not fit the {}-byte tail", encoded.len(), note_end - tail_start);
            let mut frame = original.clone();
            frame[at..at + encoded.len()].copy_from_slice(&encoded);
            fs::write(dir.frame(seq + 1), frame).unwrap();
        }
        println!("zero tail {} bytes, largest block {} bytes", note_end - tail_start, largest);
        let (_, source_dng) = read_frame(Path::new(&source)).unwrap_or_else(|_| {
            // The untouched frame has no block, so read only its DNG metadata.
            let mut tiff = TiffReader::open(Path::new(&source)).unwrap();
            let root_offset = tiff.root_ifd_offset().unwrap();
            let root = tiff.read_ifd(root_offset).unwrap();
            let exif_offset = tiff.sub_ifd_offset(find_unique(&root, EXIF_IFD_TAG, "Exif").unwrap().unwrap(), "Exif").unwrap();
            let exif = tiff.read_ifd(exif_offset).unwrap();
            (FramePayload::Protobuf(vec![]), read_dng_info(&mut tiff, &root, &exif))
        });
        println!("source WarpRectilinear green {:?}, focus {:?} m, lens {:?}", source_dng.warp_rectilinear, source_dng.subject_distance_m, source_dng.lens_model);
        assert!(source_dng.warp_rectilinear.is_some(), "a real fp frame carries WarpRectilinear");

        let sequence = read_embedded_protobuf_sequence(&dir.frame(1)).unwrap();
        println!("frames {} size {}x{} fps {:.3}", sequence.frame_count, sequence.width, sequence.height, sequence.fps);
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let virtual_url = gyroflow_core::filesystem::path_to_url(&dir.0.join("t.jsonl").to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &virtual_url, Some(metadata), true).unwrap();
        let gyro = manager.gyro.read();
        let file_metadata = gyro.file_metadata.read();
        println!("raw_imu {} quaternions {} readout {:?} ms camera {:?}",
            file_metadata.raw_imu.len(), gyro.quaternions.len(), file_metadata.frame_readout_time, file_metadata.camera_identifier);
        assert_eq!(file_metadata.per_frame_time_offsets.len(), frames as usize);
        assert_eq!(file_metadata.raw_imu.len(), (frames * per_frame + 4) as usize);
        assert!(gyro.quaternions.values().all(|q| q.coords.iter().all(|c| c.is_finite())));
        let first = file_metadata.lens_params.values().next().cloned();
        println!("lens params frame 1: {first:?}");
        drop(file_metadata);
        drop(gyro);
        check_lens_reaches_core(&manager, frames as usize);
    }

    /// Gyroflow's own autosync on a recorded clip, skipped by default:
    /// `FP_CLIP_DIR=.../A001_016 FP_GRAY=gray.raw cargo test real_fp_autosync -- --ignored --nocapture`
    /// FP_GRAY holds the clip's frames as 8-bit gray, header u32 n, w, h. FP_SHIFT
    /// feeds each image one frame late (to tell the sign of the result).
    /// The offsets it prints are what the Vd timing is still off by.
    #[test]
    #[ignore]
    fn real_fp_autosync() {
        use std::sync::{Arc, Mutex, atomic::AtomicBool};
        use gyroflow_core::synchronization::{AutosyncProcess, AutosyncResult, SyncParams};
        let dir = std::env::var("FP_CLIP_DIR").expect("FP_CLIP_DIR");
        let raw = fs::read(std::env::var("FP_GRAY").expect("FP_GRAY")).unwrap();
        let shift = std::env::var("FP_SHIFT").is_ok() as usize;
        let mut names: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dng"))).collect();
        names.sort();
        let sequence = read_embedded_protobuf_sequence(&names[0]).unwrap();
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width, height: sequence.height, fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps, rotation: sequence.rotation,
        };
        let url = gyroflow_core::filesystem::path_to_url(&std::path::Path::new(&dir).join("t.jsonl").to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &url, Some(metadata), true).unwrap();
        manager.recompute_blocking();

        let n = u32::from_le_bytes(raw[0..4].try_into().unwrap()) as usize;
        let (w, h) = (u32::from_le_bytes(raw[4..8].try_into().unwrap()), u32::from_le_bytes(raw[8..12].try_into().unwrap()));
        let frame = (w * h) as usize;
        let params = SyncParams {
            initial_offset: 0.0, initial_offset_inv: false, search_size: 200.0, calc_initial_fast: false,
            max_sync_points: 8, every_nth_frame: 1, time_per_syncpoint: 1500.0,
            of_method: 0, offset_method: 2, pose_method: 1,
            custom_sync_pattern: serde_json::Value::Null, auto_sync_points: false,
        };
        let points: Vec<f64> = (1..=8).map(|i| i as f64 / 9.0).collect();
        let mut process = AutosyncProcess::from_manager(&manager, &points, params, "synchronize".into(), Arc::new(AtomicBool::new(false))).unwrap();
        let result = Arc::new(Mutex::new(None));
        let sink = result.clone();
        process.on_finished(move |r| { *sink.lock().unwrap() = Some(r); });
        for k in 0..n.min(sequence.frame_count) {
            let image = k.saturating_sub(shift);
            let pixels = &raw[12 + image * frame..12 + (image + 1) * frame];
            let ts = (k as f64 * 1e6 / sequence.fps).round() as i64;
            process.feed_frame(ts, k, w, h, w as usize, pixels);
        }
        process.finished_feeding_frames();
        match result.lock().unwrap().take() {
            Some(AutosyncResult::Offsets(offsets)) => {
                for (ts, offset, cost) in &offsets { println!("sync point {:.0} ms: offset {offset:+.3} ms, cost {cost:.4}", ts); }
                let mut o: Vec<f64> = offsets.iter().map(|x| x.1).collect();
                o.sort_by(f64::total_cmp);
                if !o.is_empty() { println!("median offset {:+.3} ms over {} points (shift {shift})", o[o.len() / 2], o.len()); }
            }
            _ => println!("no offsets"),
        }
    }

    /// A recorded clip, skipped by default:
    /// `FP_CLIP_DIR=/path/A001_031 cargo test real_fp_clip -- --ignored --nocapture`
    /// Reads every frame's FSG2 record (the files may be just their headers),
    /// converts the take and loads it into gyroflow-core.
    #[test]
    #[ignore]
    fn real_fp_clip_dir() {
        let dir = std::env::var("FP_CLIP_DIR").expect("set FP_CLIP_DIR to a clip folder");
        let mut names: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dng")))
            .collect();
        names.sort();
        let first = names.first().expect("no DNG in FP_CLIP_DIR");
        let sequence = read_embedded_protobuf_sequence(first).unwrap();
        println!("frames {} size {}x{} fps {:.3} jsonl {} bytes", sequence.frame_count, sequence.width, sequence.height, sequence.fps, sequence.jsonl.len());
        let metadata = gyroflow_core::telemetry_parser::util::VideoMetadata {
            width: sequence.width,
            height: sequence.height,
            fps: sequence.fps,
            duration_s: sequence.frame_count as f64 / sequence.fps,
            rotation: sequence.rotation,
        };
        let url = gyroflow_core::filesystem::path_to_url(&std::path::Path::new(&dir).join("t.jsonl").to_string_lossy());
        let mut stream = std::io::Cursor::new(sequence.jsonl.as_slice());
        let manager = gyroflow_core::StabilizationManager::default();
        manager.load_video_file(&mut stream, sequence.jsonl.len() as u64, &url, Some(metadata), true).unwrap();
        let gyro = manager.gyro.read();
        let md = gyro.file_metadata.read();
        let imu = &md.raw_imu;
        let span_s = imu.last().map_or(0.0, |l| l.timestamp_ms - imu[0].timestamp_ms) / 1000.0;
        let max_dps = imu.iter().filter_map(|s| s.gyro).map(|g| g.iter().fold(0.0_f64, |m, v| m.max(v.abs()))).fold(0.0, f64::max);
        println!("raw_imu {} over {:.3} s = {:.1} Hz, max |gyro| {:.2} deg/s, frame offsets {}, quaternions {}, readout {:?} ms",
            imu.len(), span_s, (imu.len().saturating_sub(1)) as f64 / span_s.max(1e-9), max_dps,
            md.per_frame_time_offsets.len(), gyro.quaternions.len(), md.frame_readout_time);
        let first_lens = md.lens_params.values().next().cloned();
        println!("lens params first: {:?}", first_lens.map(|l| (l.focal_length, l.pixel_focal_length, l.distortion_coefficients, l.focus_distance)));
        let accel = imu.iter().filter_map(|s| s.accl).find(|a| a.iter().any(|v| *v != 0.0));
        println!("first nonzero accel (m/s^2, gyro axes): {accel:?}");
        assert_eq!(md.per_frame_time_offsets.len(), sequence.frame_count);
        assert!(gyro.quaternions.values().all(|q| q.coords.iter().all(|c| c.is_finite())));
    }
}
