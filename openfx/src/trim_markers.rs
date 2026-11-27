//! Markers on the trim points of the loaded Gyroflow project, in DaVinci Resolve: on the clips of the project's video, or
//! on the timeline's ruler where those clips show the trim points. The plugin can't reach the timeline, so a script run
//! through fuscript makes them (`trim_markers.lua`), the way "Load for current file" asks Resolve for the current clip

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{ AtomicBool, AtomicU64, Ordering::SeqCst };
use ofx::*;
use gyroflow_plugin_base::{ StabilizationManager, rfd };
use gyroflow_plugin_base::parking_lot::Mutex;
use super::fuscript::CurrentFileInfo;

const GROUP:      &str = "TrimMarkersGroup";
const ENABLED:    &str = "TrimMarkers";
const COLOR:      &str = "TrimMarkerColor";
const PICK_COLOR: &str = "TrimMarkerPickColor";
const TARGET:     &str = "TrimMarkerTarget";

/// The marker colors of Resolve, and what they look like
const COLORS: [(&str, &str); 16] = [
    ("Blue", "#007FE3"), ("Cyan", "#00CED0"), ("Green", "#00AD00"), ("Yellow", "#F09D00"),
    ("Red", "#E12401"), ("Pink", "#FF44C8"), ("Purple", "#9013FE"), ("Fuchsia", "#C02E6F"),
    ("Rose", "#FFA1B9"), ("Lavender", "#A193C8"), ("Sky", "#92E2FD"), ("Mint", "#72DB00"),
    ("Lemon", "#DCE95A"), ("Sand", "#C4915E"), ("Cocoa", "#6E5143"), ("Cream", "#F5EBE1"),
];
const DEFAULT_COLOR: usize = 7; // Fuchsia
const TARGETS: [&str; 2] = ["Clip", "Timeline"];

const FAILED_MSG: &str = "The trim markers couldn't be added. This feature relies on external scripting and is only available in paid Resolve Studio. You have to allow executing scripts:\n
Set \"Preferences -> General -> External scripting using\" to \"Local\".";

/// These are settings, with nothing in them to animate
fn not_animated<P: AsProperties + Clone>(param: &mut P) {
    let _ = <Int as RawSetter<P>>::set_at(param, c"OfxParamPropAnimates".as_ptr(), 0, &0);
}

fn color_label(color: usize) -> String {
    format!("Color: {}", COLORS[color.min(COLORS.len() - 1)].0)
}

/// Whether `name` is one of the parameters defined here
pub fn is_param(name: &str) -> bool {
    [ENABLED, COLOR, PICK_COLOR, TARGET].contains(&name)
}
/// Whether `name` is the button that opens the palette of colors
pub fn is_palette_button(name: &str) -> bool {
    name == PICK_COLOR
}

/// Defines the parameters where the markers can be made: in Resolve, which has fuscript
pub fn define(param_set: &mut ParamSetHandle) -> Result<()> {
    if !CurrentFileInfo::is_available() {
        return Ok(());
    }
    let mut group = param_set.param_define_group(GROUP)?;
    group.set_label("Trim markers")?;
    group.set_group_open(false)?;

    let mut enabled = param_set.param_define_boolean(ENABLED)?;
    enabled.set_label("Add trim markers")?;
    enabled.set_hint("When a Gyroflow project is loaded, add markers at the trim in and trim out points saved in it. A project without a trim range adds none.")?;
    enabled.set_default(false)?;
    let _ = enabled.set_script_name(ENABLED);
    enabled.set_parent(GROUP)?;
    not_animated(&mut enabled);

    // The color is picked from a palette of the marker colors where the plugin is a Fusion node, which the palette's
    // script can set it on, and from this list elsewhere (see `TrimMarkerParams::pick_with_palette`)
    let mut color = param_set.param_define_choice(COLOR)?;
    color.set_label("Color")?;
    color.set_hint("Color of the trim markers")?;
    color.set_default(DEFAULT_COLOR as Int)?;
    color.set_choices(&COLORS.map(|x| x.0))?;
    let _ = color.set_script_name(COLOR);
    color.set_parent(GROUP)?;
    not_animated(&mut color);

    let mut pick_color = param_set.param_define_button(PICK_COLOR)?;
    pick_color.set_label(&color_label(DEFAULT_COLOR))?;
    pick_color.set_hint("Pick the color of the trim markers")?;
    let _ = pick_color.set_script_name(PICK_COLOR);
    pick_color.set_parent(GROUP)?;
    pick_color.set_secret(true)?;

    let mut target = param_set.param_define_choice(TARGET)?;
    target.set_label("Place on")?;
    target.set_hint("Clip: the markers are on the clip and move with it. Timeline: they are on the timeline ruler, where the clip shows the trim points.")?;
    target.set_default(0)?;
    target.set_choices(&TARGETS)?;
    let _ = target.set_script_name(TARGET);
    target.set_parent(GROUP)?;
    not_animated(&mut target);

    Ok(())
}

/// The parameters of an instance
pub struct TrimMarkerParams {
    enabled:    ParamHandle<bool>,
    color:      ParamHandle<Int>,
    pick_color: ParamHandle<String>,
    target:     ParamHandle<Int>,
}
impl TrimMarkerParams {
    /// `None` where they aren't defined (see `define`)
    pub fn new(param_set: &ParamSetHandle) -> Option<Self> {
        let mut params = Self {
            enabled:    param_set.parameter(ENABLED).ok()?,
            color:      param_set.parameter(COLOR).ok()?,
            pick_color: param_set.parameter(PICK_COLOR).ok()?,
            target:     param_set.parameter(TARGET).ok()?,
        };
        params.update();
        Some(params)
    }

    fn color(&self) -> usize {
        (self.color.get_value().unwrap_or_default().max(0) as usize).min(COLORS.len() - 1)
    }

    /// The color and the place are for the markers that are added, and the button of the palette says the color
    pub fn update(&mut self) {
        let on = self.enabled.get_value().unwrap_or_default();
        let _ = self.color.set_enabled(on);
        let _ = self.pick_color.set_enabled(on);
        let _ = self.target.set_enabled(on);
        let _ = self.pick_color.set_label(&color_label(self.color()));
    }

    /// Shows the button of the palette in place of the list of colors
    pub fn pick_with_palette(&mut self) {
        let _ = self.color.set_secret(true);
        let _ = self.pick_color.set_secret(false);
    }

    /// Opens the palette, the marker colors as dots like in the marker dialog of Resolve, for the node with this instance
    /// id. Its script (`trim_marker_color.lua`) sets the color that is picked on the node
    pub fn pick_color(&self, instance: &str) {
        // One at a time: a press while it is opening doesn't open another
        if PALETTE_OPEN.swap(true, SeqCst) {
            return;
        }
        let script = palette_script(self.color(), instance, mouse(), COUNTER.fetch_add(1, SeqCst));
        std::thread::spawn(move || {
            // It closes by itself after a minute
            let reached_resolve = run("palette", &script, 90);
            PALETTE_OPEN.store(false, SeqCst);
            report(reached_resolve);
        });
    }

    /// The markers to make for the project `stab` has loaded, of the video file `video` gives. `None` when they are off,
    /// or when the project has no trim range
    pub fn request(&self, stab: &StabilizationManager, video: impl FnOnce() -> String) -> Option<Request> {
        if !self.enabled.get_value().unwrap_or_default() {
            return None;
        }
        let (ranges, num_frames) = {
            let params = stab.params.read();
            (trim_frames(&params.trim_ranges, params.duration_ms, params.fps, params.frame_count), params.frame_count)
        };
        if ranges.is_empty() {
            return None;
        }
        let video = video();
        if video.is_empty() {
            return None;
        }
        Some(Request {
            video,
            num_frames,
            ranges,
            color: COLORS[self.color()].0,
            on_timeline: self.target.get_value().unwrap_or_default() == 1,
        })
    }
}

/// The trim ranges of a project in frames of its video (trim in, trim out), from the ones the core keeps as fractions of
/// the duration. A range that is the whole video isn't a trim
pub fn trim_frames(trim_ranges: &[(f64, f64)], duration_ms: f64, fps: f64, frame_count: usize) -> Vec<(usize, usize)> {
    let frame = |x: f64| {
        let frame = if fps > 0.0 { x * duration_ms * fps / 1000.0 } else { x * frame_count as f64 };
        let frame = frame.round().max(0.0) as usize;
        if frame_count > 0 { frame.min(frame_count - 1) } else { frame }
    };
    trim_ranges.iter()
        .filter(|(start, end)| end > start && !(*start <= 0.0 && *end >= 1.0))
        .map(|(start, end)| (frame(*start), frame(*end)))
        .collect()
}

/// `s` as a Lua string
fn lua_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for b in s.bytes() {
        match b {
            b'"' | b'\\' => { out.push('\\'); out.push(b as char); }
            0x20..=0x7e  => out.push(b as char),
            _            => out.push_str(&format!("\\{b:03}")),
        }
    }
    out.push('"');
    out
}

/// Where the mouse is and the screen it's on, (left, top, right, bottom) without the taskbar, in the coordinates of
/// Resolve's user interface: the screen's pixels over its scale, from the screen's corner. `None` where it can't be
/// told, the palette opens in the middle of the screen then
#[cfg(target_os = "windows")]
fn mouse() -> Option<((i32, i32), (i32, i32, i32, i32))> {
    use std::ffi::c_void;
    #[repr(C)] #[derive(Default)] struct Point { x: i32, y: i32 }
    #[repr(C)] #[derive(Default)] struct Rect { left: i32, top: i32, right: i32, bottom: i32 }
    #[repr(C)] struct MonitorInfo { size: u32, monitor: Rect, work: Rect, flags: u32 }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
        fn MonitorFromPoint(point: Point, flags: u32) -> *mut c_void;
        fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetDpiForWindow(window: *mut c_void) -> u32;
    }
    unsafe {
        let mut point = Point::default();
        if GetCursorPos(&mut point) == 0 { return None; }
        let dpi = GetDpiForWindow(GetForegroundWindow());
        let mut info = MonitorInfo { size: size_of::<MonitorInfo>() as u32, monitor: Rect::default(), work: Rect::default(), flags: 0 };
        if dpi == 0 || GetMonitorInfoW(MonitorFromPoint(Point { x: point.x, y: point.y }, 2), &mut info) == 0 { return None; } // MONITOR_DEFAULTTONEAREST
        let scale = dpi as f64 / 96.0;
        let x = |v: i32| info.monitor.left + ((v - info.monitor.left) as f64 / scale).round() as i32;
        let y = |v: i32| info.monitor.top  + ((v - info.monitor.top)  as f64 / scale).round() as i32;
        Some(((x(point.x), y(point.y)), (x(info.work.left), y(info.work.top), x(info.work.right), y(info.work.bottom))))
    }
}
#[cfg(not(target_os = "windows"))]
fn mouse() -> Option<((i32, i32), (i32, i32, i32, i32))> {
    None
}

/// The script that shows the palette with `color` marked, for the node with this instance id. `id` is the palette's own
fn palette_script(color: usize, instance: &str, mouse: Option<((i32, i32), (i32, i32, i32, i32))>, id: u64) -> String {
    let colors = COLORS.iter().map(|(name, look)| format!("{{ {}, {} }}", lua_string(name), lua_string(look))).collect::<Vec<_>>().join(", ");
    let place = mouse.map(|((x, y), (left, top, right, bottom))| format!(", cursor = {{ {x}, {y} }}, screen = {{ {left}, {top}, {right}, {bottom} }}")).unwrap_or_default();
    format!("local REQUEST = {{ colors = {{ {colors} }}, current = {color}, instance = {}, id = {id}{place} }}\n{}", lua_string(instance), include_str!("trim_marker_color.lua"))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
static WARNED: AtomicBool = AtomicBool::new(false);
static PALETTE_OPEN: AtomicBool = AtomicBool::new(false);

/// Runs a script through fuscript and waits for it, `seconds` at most: one that waits for Resolve longer than that is
/// stopped. `false` when it couldn't reach Resolve
fn run(what: &str, script: &str, seconds: u64) -> std::io::Result<bool> {
    let fuscript = CurrentFileInfo::get_fuscript().ok_or(std::io::ErrorKind::NotFound)?;
    let number = COUNTER.fetch_add(1, SeqCst);
    let file = |extension: &str| std::env::temp_dir().join(format!("gyroflow-trim-markers-{}-{number}.{extension}", std::process::id()));
    let (path, out_path, err_path) = (file("lua"), file("out"), file("err"));
    // What it prints goes to files: nothing has to read it while it runs
    let finished = (|| -> std::io::Result<()> {
        std::fs::write(&path, script)?;
        let mut cmd = std::process::Command::new(fuscript);
        #[cfg(target_os = "windows")]
        { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW
        let mut child = cmd.args(["-q", "-l", "lua"]).arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::fs::File::create(&out_path)?)
            .stderr(std::fs::File::create(&err_path)?)
            .spawn()?;
        let started = std::time::Instant::now();
        while child.try_wait()?.is_none() {
            if started.elapsed().as_secs() >= seconds {
                log::warn!("Trim markers, {what}: stopped after {seconds} s");
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(())
    })();
    let read = |path: &std::path::Path| String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
    let (stdout, stderr) = (read(&out_path), read(&err_path));
    for path in [&path, &out_path, &err_path] {
        let _ = std::fs::remove_file(path);
    }
    finished?;

    for line in stdout.lines().filter(|x| !x.trim().is_empty()) {
        log::info!("Trim markers, {what}: {line}");
    }
    // fuscript complains about a missing python2 whatever the language asked for, see `CurrentFileInfo::query`
    for line in stderr.lines().filter(|x| !x.trim().is_empty() && !(x.starts_with("sh:") && x.contains("python2:"))) {
        log::warn!("Trim markers, {what}: {line}");
    }
    Ok(!stdout.lines().any(|x| x.trim() == "error: no resolve"))
}

/// Says once why the scripts do nothing
fn report(reached_resolve: std::io::Result<bool>) {
    match reached_resolve {
        Ok(true) => { },
        Ok(false) => {
            if !WARNED.swap(true, SeqCst) {
                rfd::MessageDialog::new()
                    .set_title("Failed to add the trim markers.")
                    .set_description(FAILED_MSG)
                    .set_level(rfd::MessageLevel::Warning)
                    .show();
            }
        },
        Err(e) => log::error!("Trim markers: failed to run fuscript: {e:?}")
    }
}

/// The requests waiting to run, by video: the number of the latest one
static LATEST: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);
static RUNNING: Mutex<()> = Mutex::new(());

/// Markers to make for the trim ranges of a project
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The project's video file
    pub video: String,
    pub num_frames: usize,
    /// In frames of the video
    pub ranges: Vec<(usize, usize)>,
    pub color: &'static str,
    /// On the timeline's ruler rather than on the clips
    pub on_timeline: bool,
}
impl Request {
    /// The script that makes the markers
    fn script(&self) -> String {
        let ranges = self.ranges.iter().map(|(a, b)| format!("{{ {a}, {b} }}")).collect::<Vec<_>>().join(", ");
        format!("local REQUEST = {{ video = {}, num_frames = {}, ranges = {{ {ranges} }}, color = {}, target = {} }}\n{}",
            lua_string(&self.video),
            if self.num_frames > 0 { self.num_frames.to_string() } else { "nil".to_owned() },
            lua_string(self.color),
            lua_string(if self.on_timeline { "timeline" } else { "clip" }),
            include_str!("trim_markers.lua"))
    }

    /// Makes the markers, without waiting for them. One request runs at a time, as each replaces the markers made before
    /// for its video, and of the ones waiting for a video only the latest runs
    pub fn place(self) {
        let key = self.video.to_lowercase();
        let number = COUNTER.fetch_add(1, SeqCst);
        LATEST.lock().insert(key.clone(), number);
        std::thread::spawn(move || {
            let _running = RUNNING.lock();
            {
                let mut latest = LATEST.lock();
                if latest.get(&key) != Some(&number) { return; }
                latest.remove(&key);
            }
            log::info!("Trim markers: {self:?}");
            report(run("markers", &self.script(), 120));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_frames_are_the_frames_of_the_trim_points() {
        // 20261006_C1124.gyroflow: "trim_ranges_ms": [[3286.616, 10543.866]] of 16517 ms, 990 frames at 59.9401 fps
        let range = (3286.616 / 16517.0, 10543.866 / 16517.0);
        assert_eq!(trim_frames(&[range], 16517.0, 59.9401, 990), vec![(197, 632)]);
        // A trim out at the end of the video is its last frame
        assert_eq!(trim_frames(&[(3887.216 / 69570.0, 1.0)], 69570.0, 59.9401, 4170), vec![(233, 4169)]);
        // Several ranges, in the order of the project
        assert_eq!(trim_frames(&[(0.5, 0.75), (0.0, 0.25)], 10000.0, 25.0, 250), vec![(125, 188), (0, 63)]);
        // No frame rate: by the frame count
        assert_eq!(trim_frames(&[(0.5, 0.75)], 10000.0, 0.0, 250), vec![(125, 188)]);
        // Neither the whole video nor an empty range is a trim
        assert_eq!(trim_frames(&[(0.0, 1.0)], 10000.0, 25.0, 250), vec![]);
        assert_eq!(trim_frames(&[(0.5, 0.5), (0.75, 0.25), (f64::NAN, 1.0)], 10000.0, 25.0, 250), vec![]);
        assert_eq!(trim_frames(&[], 10000.0, 25.0, 250), vec![]);
    }

    #[test]
    fn lua_strings_keep_every_byte() {
        assert_eq!(lua_string(r"E:\2026\trip\C1304.MP4"), r#""E:\\2026\\trip\\C1304.MP4""#);
        assert_eq!(lua_string("a \"b\"\n"), r#""a \"b\"\010""#);
        assert_eq!(lua_string("/Volumes/żółw/ü.mov"), r#""/Volumes/\197\188\195\179\197\130w/\195\188.mov""#);
    }

    #[test]
    fn the_script_starts_with_the_request() {
        let request = Request { video: r"E:\trip\C1304.MP4".into(), num_frames: 12300, ranges: vec![(6000, 9000), (10500, 12000)], color: "Fuchsia", on_timeline: false };
        let script = request.script();
        assert_eq!(script.lines().next(), Some(r#"local REQUEST = { video = "E:\\trip\\C1304.MP4", num_frames = 12300, ranges = { { 6000, 9000 }, { 10500, 12000 } }, color = "Fuchsia", target = "clip" }"#));
        assert!(script.ends_with(include_str!("trim_markers.lua")));

        let request = Request { video: "/a.mov".into(), num_frames: 0, ranges: vec![(1, 2)], color: "Sky", on_timeline: true };
        assert_eq!(request.script().lines().next(), Some(r#"local REQUEST = { video = "/a.mov", num_frames = nil, ranges = { { 1, 2 } }, color = "Sky", target = "timeline" }"#));
    }

    #[test]
    fn the_palette_script_starts_with_the_colors_and_the_place() {
        let script = palette_script(10, "1234", Some(((2900, 800), (0, 0, 3072, 1680))), 5);
        let first = script.lines().next().unwrap();
        assert!(first.starts_with(r##"local REQUEST = { colors = { { "Blue", "#007FE3" }, { "Cyan", "#00CED0" }, "##), "{first}");
        assert!(first.ends_with(r##"{ "Cream", "#F5EBE1" } }, current = 10, instance = "1234", id = 5, cursor = { 2900, 800 }, screen = { 0, 0, 3072, 1680 } }"##), "{first}");
        assert!(script.ends_with(include_str!("trim_marker_color.lua")));
        // Nowhere to put it: the script puts it in the middle of the screen
        assert!(palette_script(7, "1234", None, 6).lines().next().unwrap().ends_with(r#"current = 7, instance = "1234", id = 6 }"#));
        assert_eq!(COLORS[DEFAULT_COLOR].0, "Fuchsia");
        assert_eq!(color_label(10), "Color: Sky");
    }
}
