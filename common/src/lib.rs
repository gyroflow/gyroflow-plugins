
use lru::LruCache;
use parking_lot::{ Mutex, RwLock };
use std::sync::{ Arc, atomic::AtomicBool };

pub use gyroflow_core::{ StabilizationManager, keyframes::*, stabilization::*, filesystem, gpu::* };
pub use gyroflow_core;

// re-exports
pub use rfd;
pub use parking_lot;
pub use lru;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use metal;

pub type PluginResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Copy, Clone, Hash, PartialEq, PartialOrd, Eq, Ord, serde::Serialize, serde::Deserialize)]
pub enum Params {
    InstanceId,
    ProjectData,
    EmbeddedLensProfile,
    EmbeddedPreset,
    ProjectGroup, ProjectGroupEnd,
    LoadCurrent,
    ProjectPath,
    Browse,
    LoadLens,
    OpenGyroflow,
    ReloadProject,
    OpenRecentProject,
    Status,
    AdjustGroup, AdjustGroupEnd,
    Fov,
    Smoothness,
    LensCorrectionStrength,
    HorizonLockAmount,
    HorizonLockRoll,
    PositionX,
    PositionY,
    InputRotation,
    Rotation,
    VideoSpeed,
    DisableStretch,
    KeyframesGroup, KeyframesGroupEnd,
    UseGyroflowsKeyframes,
    RecalculateKeyframes,
    ToggleOverview,
    DontDrawOutside,
    IncludeProjectData,
}

pub struct GyroflowPluginBase {
    // We should cache managers globally because it's common to have the effect applied to the same clip and cut the clip into multiple pieces
    // We don't want to create a new manager for each piece of the same clip
    // Cache key is specific enough
    pub manager_cache: Mutex<LruCache<String, Arc<StabilizationManager>>>,

    pub context_initialized: bool,
    pub log_initialized: bool,
}
impl Default for GyroflowPluginBase {
    fn default() -> Self {
        Self {
            manager_cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap())),
            context_initialized: false,
            log_initialized: false,
        }
    }
}

impl GyroflowPluginBase {
    pub fn initialize_gpu_context(&mut self) {
        log::info!("GyroflowPluginBase::initialize_gpu_context");
        if !self.context_initialized {
            gyroflow_core::gpu::initialize_contexts();
            self.context_initialized = true;
        }
    }
    pub fn deinitialize_gpu_context(&mut self) {
        log::info!("GyroflowPluginBase::deinitialize_gpu_context");
    }

    pub fn initialize_log(&mut self) {
        if !self.log_initialized {
            log_panics::init();
            win_dbg_logger::init();
            log::set_max_level(log::LevelFilter::Debug);
            win_dbg_logger::output_debug_string("GyroflowPluginBase::initialize_log");
            self.log_initialized = true;

            /*let mut tmp_log = std::env::temp_dir();
            tmp_log.push("gyroflow-ofx.log");

            let log_path = if let Ok(path) = effect_properties.get_file_path() {
                std::path::Path::new(&path).with_extension("log")
            } else {
                tmp_log.clone()
            };
            let log_config = [ "mp4parse", "wgpu", "naga", "akaze", "ureq", "rustls", "ofx" ]
                .into_iter()
                .fold(simplelog::ConfigBuilder::new(), |mut cfg, x| { cfg.add_filter_ignore_str(x); cfg })
                .build();

            if let Ok(file_log) = std::fs::File::create(&log_path) {
                let _ = simplelog::WriteLogger::init(log::LevelFilter::Debug, log_config, file_log);
                self.log_initialized = true;
            } else if let Ok(file_log) = std::fs::File::create(&tmp_log) {
                let _ = simplelog::WriteLogger::init(log::LevelFilter::Debug, log_config, file_log);
                self.log_initialized = true;
            } else if cfg!(target_os = "linux") {
                if let Ok(file_log) = std::fs::File::create("/tmp/gyroflow-ofx.log") {
                    let _ = simplelog::WriteLogger::init(log::LevelFilter::Debug, log_config, file_log);
                    self.log_initialized = true;
                } else {
                    eprintln!("Failed to create log file: {log_path:?}, {tmp_log:?}, /tmp/gyroflow-ofx.log");
                }
            }*/
        }
    }

    pub fn get_center_rect(width: usize, height: usize, org_ratio: f64) -> (usize, usize, usize, usize) {
        // If aspect ratio is different
        let new_ratio = width as f64 / height as f64;
        if (new_ratio - org_ratio).abs() > 0.1 {
            // Get center rect of original aspect ratio
            let rect = if new_ratio > org_ratio {
                ((height as f64 * org_ratio).round() as usize, height)
            } else {
                (width, (width as f64 / org_ratio).round() as usize)
            };
            (
                (width - rect.0) / 2, // x
                (height - rect.1) / 2, // y
                rect.0, // width
                rect.1 // height
            )
        } else {
            (0, 0, width, height)
        }
    }

    pub fn get_project_path(file_path: &str) -> Option<String> {
        let mut project_path = std::path::Path::new(file_path).with_extension("gyroflow");
        if !project_path.exists() {
            // Find first project path that begins with the file name
            if let Some(parent) = project_path.parent() {
                if let Ok(paths) = std::fs::read_dir(parent) {
                    if let Some(fname) = project_path.with_extension("").file_name().map(|x| x.to_string_lossy().to_string()) {
                        for path in paths {
                            if let Ok(path) = path {
                                let path_fname = path.file_name().to_string_lossy().to_string();
                                if path_fname.starts_with(&fname) && path_fname.ends_with(".gyroflow") {
                                    project_path = path.path();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        if project_path.exists() {
            Some(project_path.to_string_lossy().to_string())
        } else {
            None
        }
    }

    pub fn get_gyroflow_location() -> Option<String> {
        match gyroflow_core::util::get_setting("exeLocation") {
            Some(v) if !v.is_empty() => {
                Some(v)
            },
            _ => {
                if cfg!(target_os = "macos") && std::path::Path::new("/Applications/Gyroflow.app/Contents/MacOS/gyroflow").exists() {
                    Some("/Applications/Gyroflow.app".into())
                } else {
                    None
                }
            }
        }
    }

    pub fn open_gyroflow(project_path: Option<&str>) {
        if cfg!(target_os = "macos") {
            let mut cmd = std::process::Command::new("osascript");
            if let Some(project) = project_path {
                if !project.is_empty() {
                    cmd.args(&["-e", &format!("tell application \"Gyroflow\" to open file \"{}\"", project.replace("/", ":").trim_start_matches(':'))]);
                } else {
                    cmd.args(&["-e", "tell application \"Gyroflow\" to activate"]);
                }
            }
            let _ = cmd.output();
        } else {
            if let Some(v) = Self::get_gyroflow_location() {
                if !v.is_empty() {
                    if let Some(project) = project_path {
                        if !project.is_empty() {
                            if cfg!(target_os = "macos") {
                                let _ = std::process::Command::new("open").args(["-a", &v, "--args", "--open", &project]).spawn();
                            } else {
                                let _ = std::process::Command::new(v).args(["--open", &project]).spawn();
                            }
                        } else {
                            if cfg!(target_os = "macos") {
                                let _ = std::process::Command::new("open").args(["-a", &v]).spawn();
                            } else {
                                let _ = std::process::Command::new(v).spawn();
                            }
                        }
                    }
                }
            } else {
                rfd::MessageDialog::new().set_description("Unable to find Gyroflow app path. Make sure to run Gyroflow app at least once and that version is at least v1.4.3").show();
            }
        }
    }

    pub fn get_param_definitions() -> Vec<ParameterType> {
        vec![
            ParameterType::HiddenString { id: "InstanceId" },
            ParameterType::HiddenString { id: "ProjectData" },
            ParameterType::HiddenString { id: "EmbeddedLensProfile" },
            ParameterType::HiddenString { id: "EmbeddedPreset" },
            ParameterType::Group { id: "ProjectGroup", label: "Gyroflow project", parameters: vec![
                ParameterType::Button  { id: "LoadCurrent",       label: "Load for current file",    hint: "Try to load project file for current video file, or try to stabilize that video file directly" },
                ParameterType::TextBox { id: "gyrodata",          label: "Project file",             hint: "Project file or video file" },
                ParameterType::Button  { id: "Browse",            label: "Browse",                   hint: "Browse for the Gyroflow project file" },
                ParameterType::Button  { id: "LoadLens",          label: "Load preset/lens profile", hint: "Browse for the lens profile or a preset" },
                ParameterType::Button  { id: "OpenGyroflow",      label: "Open Gyroflow",            hint: "Open project in Gyroflow" },
                ParameterType::Button  { id: "ReloadProject",     label: "Reload project",           hint: "Reload currently loaded project" },
                ParameterType::Button  { id: "OpenRecentProject", label: "Last saved project",       hint: "Load most recently saved project in the Gyroflow app" },
                ParameterType::Text    { id: "Status",            label: "Status",                   hint: "Status" },
            ] },
            ParameterType::Group { id: "AdjustGroup", label: "Adjust parameters", parameters: vec![
                ParameterType::Slider   { id: "FOV",                    label: "FOV",                  hint: "FOV",                          min: 0.1,    max: 3.0,   default: 1.0 },
                ParameterType::Slider   { id: "Smoothness",             label: "Smoothness",           hint: "Smoothness",                   min: 0.01,   max: 3.0,   default: 0.5 },
                ParameterType::Slider   { id: "LensCorrectionStrength", label: "Lens correction",      hint: "Lens correction",              min: 0.0,    max: 100.0, default: 100.0 },
                ParameterType::Slider   { id: "HorizonLockAmount",      label: "Horizon lock",         hint: "Horizon lock amount",          min: 0.0,    max: 100.0, default: 0.0 },
                ParameterType::Slider   { id: "HorizonLockRoll",        label: "Horizon roll",         hint: "Horizon lock roll adjustment", min: -100.0, max: 100.0, default: 0.0 },
                ParameterType::Slider   { id: "PositionX",              label: "Position offset X",    hint: "Position offset X",            min: -100.0, max: 100.0, default: 0.0 },
                ParameterType::Slider   { id: "PositionY",              label: "Position offset Y",    hint: "Position offset Y",            min: -100.0, max: 100.0, default: 0.0 },
                ParameterType::Slider   { id: "InputRotation",          label: "Input rotation",       hint: "Input rotation",               min: -360.0, max: 360.0, default: 0.0 },
                ParameterType::Slider   { id: "Rotation",               label: "Video rotation",       hint: "Video rotation",               min: -360.0, max: 360.0, default: 0.0 },
                ParameterType::Slider   { id: "VideoSpeed",             label: "Video speed",          hint: "Use this slider to change video speed or keyframe it, instead of built-in speed changes in the editor", min: 0.0001, max: 1000.0, default: 100.0 },
                // TODO: don't mention "Resolve"
                ParameterType::Checkbox { id: "DisableStretch",         label: "Disable Gyroflow's stretch", hint: "If you used Input stretch in the lens profile in Gyroflow, and you de-stretched the video separately in Resolve, check this to disable Gyroflow's internal stretching.", default: false },
            ] },
            ParameterType::Group { id: "KeyframesGroup", label: "Keyframes", parameters: vec![
                ParameterType::Checkbox { id: "UseGyroflowsKeyframes", label: "Use Gyroflow's keyframes", hint: "Use internal Gyroflow's keyframes, instead of the editor ones.", default: false },
                ParameterType::Button   { id: "RecalculateKeyframes",  label: "Recalculate keyframes",    hint: "Recalculate keyframes after adjusting the splines (in Fusion mode)" },
            ] },
            ParameterType::Checkbox { id: "ToggleOverview",     label: "Stabilization overview",         hint: "Zooms out the view to see the stabilization results. Disable this before rendering.", default: false },
            ParameterType::Checkbox { id: "DontDrawOutside",    label: "Don't draw outside source clip", hint: "When clip and timeline aspect ratio don't match, draw the final image inside the source clip, instead of drawing outside it.", default: false },
            ParameterType::Checkbox { id: "IncludeProjectData", label: "Embed .gyroflow data in plugin", hint: "If you intend to share the project to someone else, the plugin can embed the Gyroflow project data including gyro data inside the video editor project. This way you don't have to share .gyroflow project files. Enabling this option will make the project bigger.", default: false },
        ]
    }
}

pub enum ParameterType {
    HiddenString { id: &'static str },
    TextBox      { id: &'static str, label: &'static str, hint: &'static str },
    Text         { id: &'static str, label: &'static str, hint: &'static str },
    Slider       { id: &'static str, label: &'static str, hint: &'static str, min: f64, max: f64, default: f64 },
    Checkbox     { id: &'static str, label: &'static str, hint: &'static str, default: bool },
    Button       { id: &'static str, label: &'static str, hint: &'static str },
    Group        { id: &'static str, label: &'static str, parameters: Vec<ParameterType> }
}

#[derive(Debug, Clone)]
pub enum TimeType {
    Frame(f64),
    Milliseconds(f64),
    Microseconds(i64),
    FrameOrMicrosecond((Option<f64>, Option<i64>))
}
pub trait GyroflowPluginParams {
    fn set_enabled(&mut self, param: Params, enabled: bool) -> PluginResult<()>;
    fn set_label(&mut self, param: Params, label: &str) -> PluginResult<()>;
    fn set_hint(&mut self, param: Params, hint: &str) -> PluginResult<()>;

    fn set_f64(&mut self, param: Params, value: f64) -> PluginResult<()>;
    fn get_f64(&self, param: Params) -> PluginResult<f64>;
    fn get_f64_at_time(&self, param: Params, time: TimeType) -> PluginResult<f64>;
    fn set_bool(&mut self, param: Params, value: bool) -> PluginResult<()>;
    fn get_bool(&self, param: Params) -> PluginResult<bool>;
    fn get_bool_at_time(&self, param: Params, time: TimeType) -> PluginResult<bool>;
    fn set_string(&mut self, param: Params, value: &str) -> PluginResult<()>;
    fn get_string(&self, param: Params) -> PluginResult<String>;

    fn is_keyframed(&self, param: Params) -> bool;
    fn get_keyframes(&self, param: Params) -> Vec<(TimeType, f64)>;
    fn clear_keyframes(&mut self, param: Params) -> PluginResult<()>;
    fn set_f64_at_time(&mut self, param: Params, time: TimeType, value: f64) -> PluginResult<()>;
}

pub struct KeyframableParams {
    pub use_gyroflows_keyframes: bool,
    pub cached_keyframes: KeyframeManager
}

pub struct GyroflowPluginBaseInstance<P: GyroflowPluginParams> {
    pub parameters: P,
    pub keyframable_params: Arc<RwLock<KeyframableParams>>,

    pub managers: LruCache<String, Arc<StabilizationManager>>,

    pub reload_values_from_project: bool,

    pub original_video_size: (usize, usize),
    pub original_output_size: (usize, usize),
    pub num_frames: usize,
    pub fps: f64,
    pub ever_changed: bool,
    pub cache_keyframes_every_frame: bool,
    pub framebuffer_inverted: bool,

    pub opencl_disabled: bool,
}

impl<P> GyroflowPluginBaseInstance<P> where P: GyroflowPluginParams {
    pub fn update_loaded_state(&mut self, loaded: bool) {
        let _ = self.parameters.set_enabled(Params::Fov, loaded);
        let _ = self.parameters.set_enabled(Params::Smoothness, loaded);
        let _ = self.parameters.set_enabled(Params::LensCorrectionStrength, loaded);
        let _ = self.parameters.set_enabled(Params::HorizonLockAmount, loaded);
        let _ = self.parameters.set_enabled(Params::HorizonLockRoll, loaded);
        let _ = self.parameters.set_enabled(Params::PositionX, loaded);
        let _ = self.parameters.set_enabled(Params::PositionY, loaded);
        let _ = self.parameters.set_enabled(Params::Rotation, loaded);
        let _ = self.parameters.set_enabled(Params::VideoSpeed, loaded);
        let _ = self.parameters.set_enabled(Params::DisableStretch, loaded);
        let _ = self.parameters.set_enabled(Params::ToggleOverview, loaded);
        let _ = self.parameters.set_enabled(Params::ReloadProject, loaded);
        let _ = self.parameters.set_label(Params::Status, if loaded { "OK" } else { "Project not loaded" });
        let _ = self.parameters.set_bool(Params::Status, loaded);
        let _ = self.parameters.set_label(Params::OpenGyroflow, if loaded { "Open in Gyroflow" } else { "Open Gyroflow" });
    }

    pub fn initialize_instance_id(&mut self) {
        if self.parameters.get_string(Params::InstanceId).unwrap_or_default().is_empty() {
            self.ever_changed = true;
            let _ = self.parameters.set_string(Params::InstanceId, &format!("{}", fastrand::u64(..)));
        }
    }

    fn set_keyframe_provider(&self, stab: &StabilizationManager) {
        let kparams = self.keyframable_params.clone();
        stab.keyframes.write().set_custom_provider(move |kf, typ, timestamp_ms| -> Option<f64> {
            let params = kparams.read();
            if params.use_gyroflows_keyframes && kf.is_keyframed_internally(typ) { return None; }
            params.cached_keyframes.value_at_video_timestamp(typ, timestamp_ms)
        });
    }
    pub fn cache_keyframes(&mut self, use_gyroflows_keyframes: bool, num_frames: usize, fps: f64) {
        let mut mgr = KeyframeManager::new();
        macro_rules! cache_key {
            ($typ:expr, $param:expr, $scale:expr) => {
                if self.parameters.is_keyframed($param) {
                    log::info!("param: {:?} is keyframed, cache_keyframes_every_frame: {}", $param, self.cache_keyframes_every_frame);
                    if self.cache_keyframes_every_frame { // Query every frame
                        for t in 0..num_frames {
                            let time = t as f64;
                            let timestamp_us = ((time / fps * 1_000_000.0)).round() as i64;

                            if let Ok(v) = self.parameters.get_f64_at_time($param, TimeType::FrameOrMicrosecond((Some(time), Some(timestamp_us)))) {
                                mgr.set(&$typ, timestamp_us, v / $scale);
                            }
                        }
                    } else {
                        // Cache only the keyframes at their timestamps
                        for (t, v) in self.parameters.get_keyframes($param) {
                            let timestamp_us = match t {
                                TimeType::FrameOrMicrosecond((Some(f), None)) |
                                TimeType::Frame(f) => ((f / fps * 1_000_000.0)).round() as i64,
                                TimeType::Milliseconds(ms) => (ms * 1_000.0).round() as i64,
                                TimeType::Microseconds(us) => us,
                                TimeType::FrameOrMicrosecond((_,    Some(timestamp_us))) => timestamp_us,
                                TimeType::FrameOrMicrosecond((None, None)) => unreachable!(),
                            };

                            mgr.set(&$typ, timestamp_us, v / $scale);
                        }
                    }
                } else {
                    log::info!("param: {:?} NOT keyframed", $param);
                    if let Ok(v) = self.parameters.get_f64($param) {
                        mgr.set(&$typ, 0, v / $scale);
                    }
                }
            };
        }
        cache_key!(KeyframeType::Fov,                       Params::Fov,                    1.0);
        cache_key!(KeyframeType::SmoothingParamSmoothness,  Params::Smoothness,             1.0);
        cache_key!(KeyframeType::LensCorrectionStrength,    Params::LensCorrectionStrength, 100.0);
        cache_key!(KeyframeType::LockHorizonAmount,         Params::HorizonLockAmount,      1.0);
        cache_key!(KeyframeType::LockHorizonRoll,           Params::HorizonLockRoll,        1.0);
        cache_key!(KeyframeType::VideoSpeed,                Params::VideoSpeed,             100.0);
        cache_key!(KeyframeType::VideoRotation,             Params::Rotation,               1.0);
        cache_key!(KeyframeType::ZoomingCenterX,            Params::PositionX,              100.0);
        cache_key!(KeyframeType::ZoomingCenterY,            Params::PositionY,              100.0);

        let mut kparams = self.keyframable_params.write();
        kparams.use_gyroflows_keyframes = use_gyroflows_keyframes;
        kparams.cached_keyframes = mgr;
    }

    pub fn stab_manager(&mut self, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, bit_depth: usize, in_size: (usize, usize), out_size: (usize, usize), open_gyroflow_if_no_data: bool) -> PluginResult<Arc<StabilizationManager>> {
        let disable_stretch = self.parameters.get_bool(Params::DisableStretch)?;

        let instance_id = self.parameters.get_string(Params::InstanceId)?;
        let path = self.parameters.get_string(Params::ProjectPath)?;
        if path.is_empty() {
            self.update_loaded_state(false);
            return Err("Path is empty".into());
        }
        let key = format!("{path}{bit_depth:?}{in_size:?}{out_size:?}{disable_stretch}{instance_id}");
        let cloned = manager_cache.lock().get(&key).map(Arc::clone);
        let stab = if let Some(stab) = cloned {
            // Cache it in this instance as well
            if !self.managers.contains(&key) {
                self.managers.put(key.to_owned(), stab.clone());
            }
            self.set_keyframe_provider(&stab);
            log::info!("key found: {key}");
            stab
        } else {
            log::info!("new stab manager for key: {key}");
            let mut stab = StabilizationManager::default();
            {
                // Find first lens profile database with loaded profiles
                let lock = manager_cache.lock();
                for (_, v) in lock.iter() {
                    if v.lens_profile_db.read().loaded {
                        stab.lens_profile_db = v.lens_profile_db.clone();
                        break;
                    }
                }
            }

            if !path.ends_with(".gyroflow") {
                // Try to load from video file
                // let mut metadata = None;
                // if path.to_ascii_lowercase().ends_with(".mxf") || path.to_ascii_lowercase().ends_with(".braw") {
                //     let lock = self.current_file_info.lock();
                //     if let Some(ref current_file) = *lock {
                //         metadata = Some(VideoMetadata {
                //             duration_s: current_file.duration_s,
                //             fps: current_file.fps,
                //             width: current_file.width,
                //             height: current_file.height,
                //             rotation: 0
                //         });
                //     }
                // }

                match stab.load_video_file(&filesystem::path_to_url(&path), None) {
                    Ok(md) => {
                        if let Ok(d) = self.parameters.get_string(Params::EmbeddedLensProfile) {
                            if !d.is_empty() {
                                if let Err(e) = stab.load_lens_profile(&d) {
                                    rfd::MessageDialog::new()
                                        .set_description(&format!("Failed to load lens profile: {e:?}"))
                                        .show();
                                }
                            }
                        }
                        if let Ok(d) = self.parameters.get_string(Params::EmbeddedPreset) {
                            if !d.is_empty() {
                                let mut is_preset = false;
                                if let Err(e) = stab.import_gyroflow_data(d.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset) {
                                    rfd::MessageDialog::new()
                                        .set_description(&format!("Failed to load preset: {e:?}"))
                                        .show();
                                }
                            }
                        }
                        if self.parameters.get_bool(Params::IncludeProjectData)? {
                            if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                self.parameters.set_string(Params::ProjectData, &data)?;
                            }
                        }
                        if md.rotation != 0 {
                            let r = ((360 - md.rotation) % 360) as f64;
                            self.parameters.set_f64(Params::InputRotation, r)?;
                            stab.params.write().video_rotation = r;
                        }
                        if !stab.gyro.read().file_metadata.has_accurate_timestamps && open_gyroflow_if_no_data {
                            GyroflowPluginBase::open_gyroflow(self.parameters.get_string(Params::ProjectPath).ok().as_deref());
                        }
                    },
                    Err(e) => {
                        let embedded_data = self.parameters.get_string(Params::ProjectData)?;
                        if !embedded_data.is_empty() {
                            let mut is_preset = false;
                            stab.import_gyroflow_data(embedded_data.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset).map_err(|e| {
                                self.update_loaded_state(false);
                                format!("load_gyro_data error: {e}")
                            })?;
                        } else {
                            log::error!("An error occured: {e:?}");
                            self.update_loaded_state(false);
                            self.parameters.set_label(Params::Status, "Failed to load file info!")?;
                            self.parameters.set_hint(Params::Status, &format!("Error loading {path}: {e:?}."))?;
                            if open_gyroflow_if_no_data {
                                GyroflowPluginBase::open_gyroflow(self.parameters.get_string(Params::ProjectPath).ok().as_deref());
                            }
                            return Err(e.into());
                        }
                    }
                }
            } else {
                let project_data = {
                    if self.parameters.get_bool(Params::IncludeProjectData)? && !self.parameters.get_string(Params::ProjectData)?.is_empty() {
                        self.parameters.get_string(Params::ProjectData)?
                    } else if let Ok(data) = std::fs::read_to_string(&path) {
                        if self.parameters.get_bool(Params::IncludeProjectData)? {
                            self.parameters.set_string(Params::ProjectData, &data)?;
                        } else {
                            self.parameters.set_string(Params::ProjectData, "")?;
                        }
                        data
                    } else {
                        "".to_string()
                    }
                };
                let mut is_preset = false;
                stab.import_gyroflow_data(project_data.as_bytes(), true, Some(&filesystem::path_to_url(&path)), |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset).map_err(|e| {
                    self.update_loaded_state(false);
                    format!("load_gyro_data error: {e}")
                })?;
            }

            let loaded = {
                stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), false, true);
                let params = stab.params.read();
                self.original_video_size = params.video_size;
                self.original_output_size = params.video_output_size;
                self.num_frames = params.frame_count;
                self.fps = params.fps;
                let loaded = params.duration_ms > 0.0;
                if loaded && self.reload_values_from_project {
                    self.reload_values_from_project = false;
                    let smooth = stab.smoothing.read();
                    let smoothness = smooth.current().get_parameter("smoothness");

                    self.parameters.set_f64(Params::Fov,                    params.fov)?;
                    self.parameters.set_f64(Params::Smoothness,             smoothness)?;
                    self.parameters.set_f64(Params::LensCorrectionStrength, (params.lens_correction_amount * 100.0).min(100.0))?;
                    self.parameters.set_f64(Params::HorizonLockAmount,      if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonlockpercent } else { 0.0 })?;
                    self.parameters.set_f64(Params::HorizonLockRoll,        if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonroll } else { 0.0 })?;
                    self.parameters.set_f64(Params::VideoSpeed,             params.video_speed * 100.0)?;
                    self.parameters.set_f64(Params::PositionX,              params.adaptive_zoom_center_offset.0 * 100.0)?;
                    self.parameters.set_f64(Params::PositionY,              params.adaptive_zoom_center_offset.1 * 100.0)?;
                    self.parameters.set_f64(Params::Rotation,               params.video_rotation)?;

                    let keyframes = stab.keyframes.read();
                    let all_keys = keyframes.get_all_keys();
                    self.parameters.set_bool(Params::UseGyroflowsKeyframes, !all_keys.is_empty())?;
                    for k in all_keys {
                        if let Some(keys) = keyframes.get_keyframes(k) {
                            if !keys.is_empty() {
                                macro_rules! set_keys {
                                    ($name:expr, $scale:expr) => {
                                        self.parameters.clear_keyframes($name)?;
                                        for (ts, v) in keys {
                                            let ts = if k == &KeyframeType::VideoSpeed { params.get_source_timestamp_at_ramped_timestamp(*ts) } else { *ts };
                                            let time = (((ts as f64 / 1000.0) * params.fps) / 1000.0).round();
                                            self.parameters.set_f64_at_time($name, TimeType::Frame(time), v.value * $scale)?;
                                        }
                                    };
                                }
                                match k {
                                    KeyframeType::Fov                      => { set_keys!(Params::Fov,                    1.0); },
                                    KeyframeType::SmoothingParamSmoothness => { set_keys!(Params::Smoothness,             1.0); },
                                    KeyframeType::LensCorrectionStrength   => { set_keys!(Params::LensCorrectionStrength, 100.0); },
                                    KeyframeType::LockHorizonAmount        => { set_keys!(Params::HorizonLockAmount,      1.0); },
                                    KeyframeType::LockHorizonRoll          => { set_keys!(Params::HorizonLockRoll,        1.0); },
                                    KeyframeType::VideoSpeed               => { set_keys!(Params::VideoSpeed,             100.0); },
                                    KeyframeType::VideoRotation            => { set_keys!(Params::Rotation,               1.0); },
                                    KeyframeType::ZoomingCenterX           => { set_keys!(Params::PositionX,              100.0); },
                                    KeyframeType::ZoomingCenterY           => { set_keys!(Params::PositionY,              100.0); },
                                    _ => { }
                                }
                            }
                        }
                    }
                }
                let use_gyroflows_keyframes = self.parameters.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                self.cache_keyframes(use_gyroflows_keyframes, self.num_frames, self.fps.max(1.0));
                loaded
            };

            self.update_loaded_state(loaded);

            if disable_stretch {
                stab.disable_lens_stretch();
            }

            stab.set_fov_overview(self.parameters.get_bool(Params::ToggleOverview)?);

            let video_size = {
                let mut params = stab.params.write();
                params.framebuffer_inverted = self.framebuffer_inverted;
                params.video_size
            };

            let org_ratio = video_size.0 as f64 / video_size.1 as f64;

            let src_rect = GyroflowPluginBase::get_center_rect(in_size.0, in_size.1, org_ratio);
            stab.set_size(src_rect.2, src_rect.3);
            stab.set_output_size(out_size.0, out_size.1);

            {
                let mut stab = stab.stabilization.write();
                stab.share_wgpu_instances = true;
                stab.interpolation = gyroflow_core::stabilization::Interpolation::Lanczos4;
            }

            self.set_keyframe_provider(&stab);

            stab.invalidate_smoothing();
            stab.recompute_blocking();
            let inverse = !(self.parameters.get_bool(Params::UseGyroflowsKeyframes)? && stab.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
            stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), inverse, inverse);

            let stab = Arc::new(stab);
            // Insert to static global cache
            manager_cache.lock().put(key.to_owned(), stab.clone());
            // Cache it in this instance as well
            self.managers.put(key.to_owned(), stab.clone());

            stab
        };

        Ok(stab)
    }

    pub fn clear_stab(&mut self, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>) {
        let local_keys = self.managers.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        self.managers.clear();

        // If there are no more local references, delete it from global cache
        let mut lock = manager_cache.lock();
        for key in local_keys {
            if let Some(v) = lock.get(&key) {
                if Arc::strong_count(v) == 1 {
                    lock.pop(&key);
                }
            }
        }
    }

    pub fn disable_opencl(&mut self) {
        if !self.opencl_disabled {
            std::env::set_var("NO_OPENCL", "1");
            self.opencl_disabled = true;
        }
    }

    pub fn set_status(&mut self, status: &str, hint: &str, ok: bool) {
        let _ = self.parameters.set_label(Params::Status, status);
        let _ = self.parameters.set_hint(Params::Status, hint);
        if self.parameters.get_bool(Params::Status).unwrap_or_default() != ok {
            let _ = self.parameters.set_bool(Params::Status, ok);
            if ok {
                self.update_loaded_state(ok);
            }
        }
    }

    pub fn param_changed(&mut self, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, param: Params, user_edited: bool) -> Result<(), Box<dyn std::error::Error>> {
        if param == Params::Browse {
            let mut d = rfd::FileDialog::new()
                .add_filter("Gyroflow project files", &["gyroflow"])
                .add_filter("Video files", &["mp4", "mov", "mxf", "braw", "r3d", "insv"]);
            let current_path = self.parameters.get_string(Params::ProjectPath)?;
            if !current_path.is_empty() {
                if let Some(path) = std::path::Path::new(&current_path).parent() {
                    d = d.set_directory(path);
                }
            }
            if let Some(d) = d.pick_file() {
                self.parameters.set_string(Params::ProjectPath, &d.display().to_string())?;
            }
        }
        if param == Params::LoadLens {
            let lens_directory = || -> Option<std::path::PathBuf> {
                let exe = GyroflowPluginBase::get_gyroflow_location()?;
                if cfg!(target_os = "macos") {
                    let mut path = std::path::Path::new(&exe).to_path_buf();
                    path.push("Contents");
                    path.push("Resources");
                    path.push("camera_presets");
                    Some(path.into())
                } else {
                    let mut path = std::path::Path::new(&exe).parent()?.to_path_buf();
                    path.push("camera_presets");
                    Some(path.into())
                }
            }();
            log::info!("lens directory: {lens_directory:?}");

            let mut d = rfd::FileDialog::new().add_filter("Lens profiles and presets", &["json", "gyroflow"]);
            if let Some(dir) = lens_directory {
                d = d.set_directory(dir);
            }
            if let Some(d) = d.pick_file() {
                let d = d.display().to_string();
                if !d.is_empty() {
                    if let Ok(contents) = std::fs::read_to_string(&d) {
                        if d.ends_with(".json") {
                            self.parameters.set_string(Params::EmbeddedLensProfile, &contents)?;
                        } else {
                            self.parameters.set_string(Params::EmbeddedPreset, &contents)?;
                        }
                    }
                    self.clear_stab(&manager_cache);
                }
            }
        }
        if param == Params::OpenGyroflow {
            GyroflowPluginBase::open_gyroflow(self.parameters.get_string(Params::ProjectPath).ok().as_deref());
        }
        if param == Params::OpenRecentProject {
            let last_project = /*if cfg!(target_os = "macos") {
                let mut cmd = std::process::Command::new("defaults");
                cmd.args(&["read", "com.gyroflow-xyz.Gyroflow", "lastProject"]);
                cmd.output().ok().map(|x| String::from_utf8_lossy(&x.stdout).to_string())
            } else */{
                gyroflow_core::util::get_setting("lastProject")
            };
            if let Some(v) = last_project {
                if !v.is_empty() {
                    self.parameters.set_string(Params::ProjectPath, &v)?;
                }
            }
        }
        if param == Params::ProjectPath || param == Params::ReloadProject || param == Params::DontDrawOutside {
            if param == Params::ProjectPath || param == Params::ReloadProject {
                self.reload_values_from_project = true;
            }
            self.clear_stab(&manager_cache);
        }
        if param == Params::IncludeProjectData {
            let path = self.parameters.get_string(Params::ProjectPath)?;
            if self.parameters.get_bool(Params::IncludeProjectData).unwrap_or_default() {
                if path.ends_with(".gyroflow") {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if StabilizationManager::project_has_motion_data(data.as_bytes()) {
                            self.parameters.set_string(Params::ProjectData, &data)?;
                        } else {
                            if let Some((_, stab)) = self.managers.peek_lru() {
                                if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                    self.parameters.set_string(Params::ProjectData, &data)?;
                                }
                            }
                        }
                    } else {
                        self.parameters.set_string(Params::ProjectData, "")?;
                    }
                } else {
                    if let Some((_, stab)) = self.managers.peek_lru() {
                        if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                            self.parameters.set_string(Params::ProjectData, &data)?;
                        }
                    }
                }
            } else {
                self.parameters.set_string(Params::ProjectData, &"")?;
            }
        }
        if user_edited {
            match param {
                Params::Fov | Params::Smoothness | Params::LensCorrectionStrength |
                Params::HorizonLockAmount | Params::HorizonLockRoll |
                Params::PositionX | Params::PositionY | Params::Rotation | Params::InputRotation | Params::VideoSpeed |
                Params::UseGyroflowsKeyframes | Params::RecalculateKeyframes =>{

                    self.parameters.set_label(Params::Status, "Calculating...")?;
                    if !self.ever_changed {
                        self.ever_changed = true;
                        self.parameters.set_string(Params::InstanceId, &format!("{}", fastrand::u64(..)))?;
                        self.clear_stab(manager_cache);
                    }
                    let use_gyroflows_keyframes = self.parameters.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                    self.cache_keyframes(use_gyroflows_keyframes, self.num_frames, self.fps.max(1.0));
                    for (_, v) in self.managers.iter_mut() {
                        match param {
                            Params::Smoothness | Params::HorizonLockAmount | Params::HorizonLockRoll | Params::RecalculateKeyframes => { v.invalidate_blocking_smoothing(); v.invalidate_blocking_zooming(); },
                            Params::LensCorrectionStrength | Params::PositionX | Params::PositionY | Params::Rotation => { v.invalidate_blocking_zooming(); },
                            _ => { }
                        }
                        v.invalidate_blocking_undistortion();
                        match param {
                            Params::VideoSpeed | Params::UseGyroflowsKeyframes | Params::RecalculateKeyframes => {
                                let inverse = !(use_gyroflows_keyframes && v.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
                                v.params.write().calculate_ramped_timestamps(&v.keyframes.read(), inverse, inverse);
                            },
                            _ => { }
                        }
                    }
                },
                _ => { }
            }
        }

        if param == Params::ToggleOverview && user_edited {
            let on = self.parameters.get_bool(Params::ToggleOverview)?;
            for (_, v) in self.managers.iter_mut() {
                v.set_fov_overview(on);
                v.invalidate_blocking_undistortion();
            }
        }

        Ok(())
    }
}

impl std::str::FromStr for Params {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gyrodata" => Ok(Self::ProjectPath),
            "FOV"      => Ok(Self::Fov),
            _ => serde_json::from_str(&format!("\"{}\"", s))
        }
    }
}
impl ToString for Params {
    fn to_string(&self) -> String {
        match self {
            Self::ProjectPath => "gyrodata".to_string(),
            Self::Fov         => "FOV".to_string(),
            _ => format!("{:?}", self)
        }
    }
}

#[macro_export]
macro_rules! define_params {
    ($name:ident {
        strings: [ $($str_enum:ident  => $str_field:ident: $str_host_type:ty,)* ],
        bools:   [ $($bool_enum:ident => $bool_field:ident: $bool_host_type:ty,)* ],
        f64s:    [ $($f64_enum:ident  => $f64_field:ident: $f64_host_type:ty,)* ],

        get_string:  $gstr_s:ident   $gstr_p:ident                    $gstr_block:block,
        set_string:  $sstr_s:ident   $sstr_p:ident,   $sstr_v:ident   $sstr_block:block,
        get_bool:    $gbool_s:ident  $gbool_p:ident                   $gbool_block:block,
        set_bool:    $sbool_s:ident  $sbool_p:ident,  $sbool_v:ident  $sbool_block:block,
        get_f64:     $gf64_s:ident   $gf64_p:ident                    $gf64_block:block,
        set_f64:     $sf64_s:ident   $sf64_p:ident,   $sf64_v:ident   $sf64_block:block,
        set_label:   $slabel_s:ident $slabel_p:ident, $slabel_v:ident $slabel_block:block,
        set_hint:    $shint_s:ident  $shint_p:ident,  $shint_v:ident  $shint_block:block,
        set_enabled: $sen_s:ident    $sen_p:ident,    $sen_v:ident    $sen_block:block,
        get_bool_at_time: $gtbool_s:ident  $gtbool_p:ident, $gtbool_t:ident                $gtbool_block:block,
        get_f64_at_time:  $gtf64_s:ident   $gtf64_p:ident,  $gtf64_t:ident                 $gtf64_block:block,
        set_f64_at_time:  $stf64_s:ident  $stf64_p:ident,  $stf64_t:ident, $stf64_v:ident $stf64_block:block,
        is_keyframed: $iskeyframe_s:ident  $iskeyframe_p:ident $iskeyframe_block:block,
        get_keyframes: $gkeyframes_s:ident $gkeyframes_p:ident $gkeyframes_block:block,
        clear_keyframes: $clr_s:ident      $clr_p:ident $clr_block:block,

        $($additional_fields:ident: $additional_fields_t:ty,)*
    }) => {
        #[derive(Default)]
        pub struct ParamsAdditionalFields {
            $( pub $additional_fields: $additional_fields_t, )*
        }
        pub struct $name {
            $( $str_field: $str_host_type, )*
            $( $bool_field: $bool_host_type, )*
            $( $f64_field: $f64_host_type, )*

            pub fields: ParamsAdditionalFields,
        }
        impl GyroflowPluginParams for $name {
            fn get_string(&self, param: Params) -> $crate::PluginResult<String> {
                let $gstr_s = &self.fields;
                match param {
                    $( Params::$str_enum => { let $gstr_p = &self.$str_field; $gstr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_string(&mut self, param: Params, value: &str) -> $crate::PluginResult<()> {
                let mut $sstr_s = &mut self.fields;
                match param {
                    $( Params::$str_enum => { let $sstr_p = &mut self.$str_field; let $sstr_v = value; $sstr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_bool(&self, param: Params) -> $crate::PluginResult<bool> {
                let $gbool_s = &self.fields;
                match param {
                    $( Params::$bool_enum => { let $gbool_p = &self.$bool_field; $gbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_bool(&mut self, param: Params, value: bool) -> $crate::PluginResult<()> {
                let mut $sbool_s = &mut self.fields;
                match param {
                    $( Params::$bool_enum => { let $sbool_p = &mut self.$bool_field; let $sbool_v = value; $sbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_f64(&self, param: Params) -> $crate::PluginResult<f64> {
                let $gf64_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gf64_p = &self.$f64_field; $gf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_f64(&mut self, param: Params, value: f64) -> $crate::PluginResult<()> {
                let mut $sf64_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $sf64_p = &mut self.$f64_field; let $sf64_v = value; $sf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_label(&mut self, param: Params, label: &str) -> $crate::PluginResult<()> {
                let mut $slabel_s = &mut self.fields;
                let $slabel_v = label;
                match param {
                    $( Params::$str_enum  => { let $slabel_p = &mut self.$str_field;  $slabel_block }, )*
                    $( Params::$bool_enum => { let $slabel_p = &mut self.$bool_field; $slabel_block }, )*
                    $( Params::$f64_enum  => { let $slabel_p = &mut self.$f64_field;  $slabel_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_hint(&mut self, param: Params, hint: &str) -> $crate::PluginResult<()> {
                let mut $shint_s = &mut self.fields;
                let $shint_v = hint;
                match param {
                    $( Params::$str_enum  => { let $shint_p = &mut self.$str_field;  $shint_block }, )*
                    $( Params::$bool_enum => { let $shint_p = &mut self.$bool_field; $shint_block }, )*
                    $( Params::$f64_enum  => { let $shint_p = &mut self.$f64_field;  $shint_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_enabled(&mut self, param: Params, enabled: bool) -> $crate::PluginResult<()> {
                let mut $sen_s = &mut self.fields;
                let $sen_v = enabled;
                match param {
                    $( Params::$str_enum  => { let $sen_p = &mut self.$str_field;  $sen_block }, )*
                    $( Params::$bool_enum => { let $sen_p = &mut self.$bool_field; $sen_block }, )*
                    $( Params::$f64_enum  => { let $sen_p = &mut self.$f64_field;  $sen_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_f64_at_time(&self, param: Params, time: TimeType) -> $crate::PluginResult<f64> {
                let $gtf64_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gtf64_p = &self.$f64_field; let $gtf64_t = time; $gtf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_bool_at_time(&self, param: Params, time: TimeType) -> $crate::PluginResult<bool> {
                let $gtbool_s = &self.fields;
                match param {
                    $( Params::$bool_enum => { let $gtbool_p = &self.$bool_field; let $gtbool_t = time; $gtbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn clear_keyframes(&mut self, param: Params) -> $crate::PluginResult<()> {
                let mut $clr_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $clr_p = &mut self.$f64_field; $clr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn is_keyframed(&self, param: Params) -> bool {
                let $iskeyframe_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $iskeyframe_p = &self.$f64_field; $iskeyframe_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_keyframes(&self, param: Params) -> Vec<(TimeType, f64)> {
                let $gkeyframes_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gkeyframes_p = &self.$f64_field; $gkeyframes_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_f64_at_time(&mut self, param: Params, time: TimeType, value: f64) -> $crate::PluginResult<()> {
                let mut $stf64_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $stf64_p = &mut self.$f64_field; let $stf64_t = time; let $stf64_v = value; $stf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
        }
    };
}
