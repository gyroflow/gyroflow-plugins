
use after_effects as ae;

use gyroflow_plugin_base::*;
use gyroflow_plugin_base::gyroflow_core::GyroflowCoreError;
use std::collections::HashSet;
use lru::LruCache;
use parking_lot::{ Mutex, RwLock };
use std::sync::Arc;
use ae::aegp::{ LayerFlags, LayerStream, TimeMode, PluginId, StreamValue };

mod ui;
mod premiere;

mod parameters;
use parameters::*;

mod sequence_data;
mod effect_instances;
use effect_instances::EffectKey;

use serde::{ Serialize, Deserialize };

static mut AEGP_PLUGIN_ID: PluginId = 0;
static mut IS_PREMIERE: bool = false;
/// After Effects' UI thread, which sets up the plugin, and its effects (see `effect_instances`)
static UI_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();
static GLOBAL_INST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
pub(crate) fn global_inst<'a>() -> &'a Plugin {
    unsafe {
        let ptr = (*GLOBAL_INST.get_or_init(|| 0)) as *const Plugin;
        &*ptr
    }
}

#[derive(Default)]
struct Plugin {
    gyroflow: GyroflowPluginBase
}

struct RenderData {
    stab: Arc<StabilizationManager>,
    stored: Arc<RwLock<StoredParams>>
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredParams {
    pub version: u8,
    pub media_file_path: String,
    pub instance_id: String,
    pub sequence_size: (usize, usize),
    pub media_fps: f64,
    pub media_fps_ticks: i64,
    pub pending_params_f64: HashMap<Params, f64>,
    pub pending_params_bool: HashMap<Params, bool>,
    pub pending_params_str: HashMap<Params, String>,
    pub pending_params_i32: HashMap<Params, i32>,
    pub premiere_keyframed_params: HashSet<Params>,
    pub speed_per_frame: Vec<f64>,
    pub speed_checksum: u64,
    /// The state of each keyframable parameter's values over time when they were last seen, see `update_keyframes`. Of
    /// this session only, not saved
    #[serde(skip)]
    pub keyframes_state: Vec<(Params, ae::sys::PF_State)>,
    /// The values the plugin set keyframable parameters to, which the host applies after the edit being handled (see
    /// `ParamHandler::changed_keyframable_params`). Of this session only, not saved
    #[serde(skip)]
    pub keyframable_set_to: HashMap<Params, f64>,
}
impl Default for StoredParams {
    fn default() -> Self {
        let instance_id = format!("{}", fastrand::u64(..));
        Self {
            version: 1,
            media_file_path: String::new(),
            instance_id: instance_id.clone(),
            sequence_size: (0, 0),
            media_fps: 0.0,
            media_fps_ticks: 0,
            pending_params_f64: HashMap::new(),
            pending_params_bool: HashMap::new(),
            pending_params_str: HashMap::from([
                (Params::Status, String::from("---")),
            ]),
            pending_params_i32: HashMap::new(),
            premiere_keyframed_params: HashSet::new(),
            speed_per_frame: Vec::new(),
            speed_checksum: 0,
            keyframes_state: Vec::new(),
            keyframable_set_to: HashMap::new(),
        }
    }
}
/// What `StoredParams::follow_media` found the clip's media to be
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaChange {
    /// The stored file
    None,
    /// The stored file, moved
    Moved,
    /// Another clip's file: the effect was copied to that clip
    OtherClip,
}

impl StoredParams {
    /// The host says the clip of the effect's own instance has the media `host_media`. Returns whether that's another
    /// file than `media_file_path`, which is then updated, for the caller to read again what it reads from the media.
    ///
    /// The effect's project, `project_path` (or `project_data` embedded of it), and the footage the effect `loaded` of the
    /// stored media tell which it is: the project was made for some footage (see `Footage`), and the effect follows media
    /// of that footage, while on other footage it's another clip's effect.
    /// - Relinked media, moved, on a volume mounted under another name, the project opened on another computer, or a
    ///   copy or a transcode of the file (named after it, of its footage at any size), is followed. The project path
    ///   follows it when it was found for the old media (the media itself, or its project next to it) and can't be loaded
    ///   anymore. One picked by the user stays, and so does one still loading from the embedded data.
    /// - Another clip's: the effect was copied to it (pasted, or applied with a preset made of it, which keeps the
    ///   sequence data), and starts over on it with its project.
    ///
    /// Media the host has offline isn't followed until it's back. When the footage can't be read (a file gone, or of a
    /// format without metadata), a file that's gone was moved if it was named the same, and one still there is another
    /// clip's unless the media is a copy of it. With nothing telling, the effect starts over on the media, as on another
    /// clip's: its project can't be read anyway, and the media's values are right either way
    pub fn follow_media(&mut self, host_media: &str, project_path: &str, project_data: &str, loaded: Option<Footage>) -> MediaChange {
        if self.has_media(host_media) || !std::path::Path::new(host_media).exists() {
            return MediaChange::None;
        }
        let stored_exists = std::path::Path::new(&self.media_file_path).exists();
        let project = if project_path.ends_with(".gyroflow") {
            std::fs::read_to_string(project_path).ok().and_then(|x| GyroflowPluginBase::project_footage(&x))
        } else {
            GyroflowPluginBase::video_footage(project_path)
        }.or_else(|| GyroflowPluginBase::project_footage(project_data));
        let host = GyroflowPluginBase::video_footage(host_media);
        let stored = if stored_exists { GyroflowPluginBase::video_footage(&self.media_file_path) } else { loaded };
        let for_project = |x: Option<Footage>| Some(project?.matches(&x?));
        let file_name = |x: &str| x.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();

        let relink = Relink {
            stored_exists,
            same_name: GyroflowPluginBase::same_file(&file_name(host_media), &file_name(&self.media_file_path)),
            named_after: is_named_after(host_media, &self.media_file_path),
            copy_of_stored: same_contents(&self.media_file_path, host_media),
            same_footage: host.zip(stored).map(|(host, stored)| host.matches_scaled(&stored)),
            for_host: for_project(host),
            for_stored: if stored_exists { for_project(stored) } else { None },
            project_known: project.is_some(),
        };
        if !relinked(relink) {
            self.move_to_clip(host_media);
            return MediaChange::OtherClip;
        }
        let old_media = std::mem::replace(&mut self.media_file_path, host_media.to_owned());

        let found_for_old_media = GyroflowPluginBase::same_file(project_path, &old_media) || is_project_of(project_path, &old_media);
        let loads = std::path::Path::new(project_path).exists() || (project_path.ends_with(".gyroflow") && !project_data.is_empty());
        if found_for_old_media && !loads {
            let new_project_path = GyroflowPluginBase::get_project_path(host_media).unwrap_or_else(|| host_media.to_owned());
            log::info!("Media moved from {old_media} to {host_media}, loading {new_project_path} instead of {project_path}");
            self.pending_params_str.insert(Params::ProjectPath, new_project_path);
        } else {
            log::info!("Media moved from {old_media} to {host_media}, keeping {project_path}");
        }
        MediaChange::Moved
    }

    /// The effect was copied to the clip of `host_media`. That clip's project is loaded, everything else that belongs to
    /// the clip starts over, and the adjustments stay (see `GyroflowPluginBaseInstance::keep_adjustments`)
    fn move_to_clip(&mut self, host_media: &str) {
        let old_media = std::mem::replace(&mut self.media_file_path, host_media.to_owned());
        let project_path = GyroflowPluginBase::get_project_path(host_media).unwrap_or_else(|| host_media.to_owned());
        log::info!("Effect copied from the clip of {old_media} to the one of {host_media}, loading {project_path}");

        // Its own stabilization, not the one of the clip it came from
        self.new_instance_id();
        self.media_fps = 0.0;
        self.media_fps_ticks = 0;
        self.speed_per_frame.clear();
        self.speed_checksum = 0;
        self.pending_params_str.insert(Params::ProjectPath, project_path);
        // The pending values shadow the saved parameters, which came with the copy as well
        self.pending_params_str.insert(Params::ProjectData, String::new());
        self.pending_params_str.remove(&Params::LoadedProject);
        for p in [Params::InputRotation, Params::Rotation, Params::OutputWidth, Params::OutputHeight] {
            self.pending_params_f64.remove(&p);
        }
        // Of the clip as well, the host sets them for this one (see `on_clip`). The sequence size is kept until then, it
        // says whether the output size followed the sequence's
        self.pending_params_bool.remove(&Params::DisableStretch);
    }

    /// The effect's clip is new to it: it's set up on it (`first`), or the effect was copied to it (see `follow_media`).
    /// The clip is in a sequence (or comp) of `sequence_size`, and its media is `anamorphic` or not: the output size
    /// follows the sequence's unless the user set one, and Disable stretch follows the media's pixel aspect, which the
    /// host stretches already
    pub fn on_clip(&mut self, sequence_size: (usize, usize), anamorphic: bool, first: bool) {
        if first || self.sequence_size != (0, 0) {
            self.sequence_size = sequence_size;
        }
        if anamorphic || !first {
            self.pending_params_bool.insert(Params::DisableStretch, anamorphic);
        }
    }

    /// Whether the effect is on the media `host_media`, as far as it knows (either can be unknown)
    pub fn has_media(&self, host_media: &str) -> bool {
        host_media.is_empty() || self.media_file_path.is_empty() || GyroflowPluginBase::same_file(host_media, &self.media_file_path)
    }

    /// Gives the effect a stabilization of its own, as the stabilizations are cached by the instance id
    fn new_instance_id(&mut self) {
        self.instance_id = format!("{}", fastrand::u64(..));
        // The pending value is set to the parameter with the next edit, and mustn't bring the one before back
        self.pending_params_str.insert(Params::InstanceId, self.instance_id.clone());
    }
}

/// Whether `project` is where `GyroflowPluginBase::get_project_path` finds the project of `media`: next to it, named
/// after it. Decided from the paths alone, as both can be gone
fn is_project_of(project: &str, media: &str) -> bool {
    let (project, media) = (std::path::Path::new(project), std::path::Path::new(media));
    let name = project.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    let media_name = media.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    !media_name.is_empty() && project.parent() == media.parent() && name.starts_with(&media_name) && name.ends_with(".gyroflow")
}

/// What tells whether the media of an effect's clip was relinked from the media stored for it, see `relinked`. `None`
/// where the footage can't be read
#[derive(Debug, Default, Clone, Copy)]
struct Relink {
    /// The stored file is still there
    stored_exists: bool,
    /// The media has the stored file's name
    same_name: bool,
    /// The media is named after the stored file (see `is_named_after`)
    named_after: bool,
    /// The media has the stored file's contents
    copy_of_stored: bool,
    /// The media has the stored media's footage, at any size (see `Footage::matches_scaled`): the stored file's, or else
    /// what the effect loaded of it
    same_footage: Option<bool>,
    /// The effect's project was made for the footage of the media
    for_host: Option<bool>,
    /// The effect's project was made for the footage of the stored file
    for_stored: Option<bool>,
    /// The footage the effect's project was made for is known: it loads
    project_known: bool,
}

/// Whether the media of an effect's clip was relinked from the media stored, rather than the effect copied to another
/// clip: the media is a copy of the stored file, or a transcode of its footage named after it (one of the same name is
/// another camera's clip as long). Or with the stored file there, the effect's project was made for the footage of the
/// media and not for the stored one's, which would be of another clip as long. Or with the stored file gone, it was
/// named the same, or the project or the stored media's footage says it's the same footage, and nothing says otherwise,
/// or the project loads and the media's footage can't be read to tell. The project's footage is as the app read it,
/// which can tell a size or a duration apart from what the plugin reads, so it doesn't overrule the name of a file
/// that's gone
fn relinked(r: Relink) -> bool {
    let transcode = r.named_after && !r.same_name && r.same_footage == Some(true);
    if r.copy_of_stored || transcode {
        return true;
    }
    if r.stored_exists {
        r.for_host == Some(true) && r.for_stored != Some(true)
    } else {
        r.same_name || r.for_host == Some(true) || (r.for_host.is_none() && (r.same_footage == Some(true) || r.project_known))
    }
}

/// Whether the file `media` is named after the file `stored`, as a transcode or a proxy of it is: the same name but for
/// the extension, or with something more after it (`C0001.MP4` to `C0001.mov` or `C0001_Proxy.mp4`, not `C00010.MP4`)
fn is_named_after(media: &str, stored: &str) -> bool {
    let stem = |x: &str| {
        let name = x.rsplit(['/', '\\']).next().unwrap_or_default();
        let stem = name.rsplit_once('.').map_or(name, |x| x.0);
        if cfg!(any(target_os = "windows", target_os = "macos")) { stem.to_lowercase() } else { stem.to_owned() }
    };
    let (media, stored) = (stem(media), stem(stored));
    !stored.is_empty() && media.strip_prefix(&stored).is_some_and(|rest| !rest.starts_with(char::is_alphanumeric))
}

/// Whether the files `a` and `b` have the same contents, as far as their size and their first and last 64 KiB tell
fn same_contents(a: &str, b: &str) -> bool {
    use std::io::{ Read, Seek, SeekFrom };
    const CHUNK: u64 = 64 * 1024;
    let read = |path: &str| -> Option<(u64, Vec<u8>)> {
        let mut file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let mut bytes = Vec::new();
        if len <= CHUNK * 2 {
            file.read_to_end(&mut bytes).ok()?;
        } else {
            (&mut file).take(CHUNK).read_to_end(&mut bytes).ok()?;
            file.seek(SeekFrom::End(-(CHUNK as i64))).ok()?;
            file.take(CHUNK).read_to_end(&mut bytes).ok()?;
        }
        Some((len, bytes))
    };
    match (read(a), read(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false
    }
}

/// The clip an After Effects layer shows, as far as the effect on it follows it (see `LayerClip::of`)
#[derive(Default)]
struct LayerClip {
    /// Its media, empty when there's none to follow
    media: String,
    /// The part of the media the layer shows, in seconds
    trim: Option<(f64, f64)>,
    /// The media is offline
    missing: bool,
    /// Of the several videos of a precomp, the first: which one is the effect's can't be told
    ambiguous: bool,
    item: Option<ae::aegp::Item>,
}
impl LayerClip {
    /// The clip of the effect on `layer`: its footage, or in the comp it shows (a precomp), the footage of the layer of
    /// the `stored` media, wherever the layers are now, or else of the only video. A still or an audio layer is no clip to
    /// stabilize, and of several videos, the effect's can't be told
    fn of(layer: &ae::aegp::Layer, stored: &str) -> Result<Self, ae::Error> {
        let of_layer = |layer: &ae::aegp::Layer, item: ae::aegp::Item, media: String| -> Result<Self, ae::Error> {
            let in_point = layer.in_point(TimeMode::LayerTime)?;
            let out_point = in_point + layer.duration(TimeMode::LayerTime)?;
            Ok(Self {
                media,
                trim: Some((in_point.into(), out_point.into())),
                missing: item.flags().is_ok_and(|x| x.contains(ae::aegp::ItemFlags::MISSING)),
                ambiguous: false,
                item: Some(item),
            })
        };
        let item = layer.source_item()?;
        match item.item_type()? {
            ae::aegp::ItemType::Footage => {
                let media = item.main_footage()?.path(0, 0)?;
                of_layer(layer, item, media)
            }
            ae::aegp::ItemType::Comp => {
                let comp = item.composition()?;
                let mut videos = Vec::new();
                for i in 0..comp.num_layers()? {
                    let Ok(layer) = comp.layer_by_index(i) else { continue; };
                    let Some(item) = layer.source_item().ok().filter(|x| x.item_type().ok() == Some(ae::aegp::ItemType::Footage)) else { continue; };
                    // A solid has no file
                    let Ok(media) = item.main_footage().and_then(|x| x.path(0, 0)) else { continue; };
                    if !stored.is_empty() && GyroflowPluginBase::same_file(&media, stored) {
                        return of_layer(&layer, item, media);
                    }
                    if item.flags().is_ok_and(|x| x.contains(ae::aegp::ItemFlags::HAS_VIDEO) && !x.contains(ae::aegp::ItemFlags::STILL)) {
                        videos.push((layer, item, media));
                    }
                }
                let ambiguous = videos.len() > 1;
                match videos.into_iter().next() {
                    Some((layer, item, media)) => Ok(Self { ambiguous, ..of_layer(&layer, item, media)? }),
                    None => Ok(Self::default()),
                }
            }
            _ => Ok(Self::default()),
        }
    }
    /// Whether its pixels aren't square, which the host stretches
    fn anamorphic(&self) -> bool {
        self.item.as_ref().and_then(|x| x.pixel_aspect_ratio().ok()).is_some_and(|x| (f64::from(x) * 100.0).round() != 100.0)
    }
}

#[derive(Serialize, Deserialize)]
struct Instance {
    #[serde(serialize_with="ser_stored", deserialize_with="de_stored")]
    stored: Arc<RwLock<StoredParams>>,
    // Has to stay the last field, see `sequence_data`
    #[serde(serialize_with="sequence_data::ser_base", deserialize_with="sequence_data::de_base")]
    gyroflow: Option<GyroflowPluginBaseInstance>
}
ae::define_cross_thread_type!(Instance);

impl CrossThreadInstance {
    fn new_base_instance(instance_id: &mut String) -> GyroflowPluginBaseInstance {
        log::info!("new_base_instance: {:?}", instance_id);
        let mut gyroflow = Self::base_instance();
        gyroflow.initialize_instance_id(instance_id);
        gyroflow
    }
    /// The base instance as set up for the Adobe hosts
    fn base_instance() -> GyroflowPluginBaseInstance {
        GyroflowPluginBaseInstance {
            managers:                       LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
            original_output_size:           (0, 0),
            original_video_size:            (0, 0),
            timeline_size:                  (0, 0),
            num_frames:                     0,
            fps:                            0.0,
            has_motion:                     false,
            reload_values_from_project:     true,
            keep_adjustments:               false,
            ever_changed:                   false,
            opencl_disabled:                false,
            cache_keyframes_every_frame:    true,
            framebuffer_inverted:           false, //unsafe { IS_PREMIERE },
            anamorphic_adjust_size:         false,
            fixed_output_size:              false,
            last_out_size:                  (0, 0),
            keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                use_gyroflows_keyframes:  false,
                cached_keyframes:         KeyframeManager::default()
            })),
        }
    }

    /// A copy of this instance, as a new instance
    fn copy(&self) -> Self {
        let new = Self::default();
        let (src, dst) = (self.get().unwrap(), new.get().unwrap());
        let (src, mut dst) = (src.read(), dst.write());
        *dst.stored.write() = src.stored.read().clone();
        dst.gyroflow = src.gyroflow.clone();
        new
    }
    /// `inst`, as a new instance
    fn insert(inst: Instance) -> Self {
        let id = fastrand::u64(..);
        Self::map().write().insert(id, Arc::new(RwLock::new(inst)));
        Self { id }
    }

    /// The instance saved in `bytes` by a build of `version`, the one in the map when it's there already
    fn restore(version: u16, bytes: &[u8]) -> Self {
        let inst = sequence_data::unflatten(version, bytes);
        if let Some(x) = inst.get().filter(|x| x.read().gyroflow.is_none()) {
            x.write().ensure_base();
        }
        inst
    }
}

impl Default for Instance {
    fn default() -> Self {
        let mut stored = StoredParams::default();
        let gyroflow = CrossThreadInstance::new_base_instance(&mut stored.instance_id);
        Self {
            stored: Arc::new(RwLock::new(stored)),
            gyroflow: Some(gyroflow),
        }
    }
}

/// Caches the keyframable parameters again when their values changed without an edit reported with `UserChangedParam`:
/// After Effects doesn't report keyframes removed with the stopwatch, moved or eased in the timeline, or an undo. A
/// stabilization shared with the effect's copies isn't for these values anymore (see `keyframable_params_changed`).
/// `reported` is the parameter of the edit being handled, cached again with it
fn update_keyframes(base: &mut GyroflowPluginBaseInstance, params: &mut ParamHandler, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, reported: Option<Params>) {
    let changed = params.changed_keyframable_params(reported);
    if !changed.is_empty() {
        log::info!("Keyframable parameters changed without an edit: {changed:?}");
        if let Err(e) = base.keyframable_params_changed(params, manager_cache, &changed) {
            log::error!("keyframable_params_changed error: {e:?}");
        }
    }
}

impl Instance {
    /// Sets up the base instance, which older builds didn't always save
    fn ensure_base(&mut self) {
        if self.gyroflow.is_none() {
            let gyroflow = CrossThreadInstance::new_base_instance(&mut self.stored.write().instance_id);
            log::info!("New base instance, instance_id: {}", self.stored.read().instance_id);
            self.gyroflow = Some(gyroflow);
        }
    }

    /// See `StoredParams::follow_media`, with the effect's `project` path and embedded data. On another clip, the effect
    /// loads that clip's project, and keeps its adjustments
    fn follow_media(&mut self, host_media: &str, project: impl FnOnce() -> (String, String)) -> MediaChange {
        if self.stored.read().has_media(host_media) {
            return MediaChange::None;
        }
        let (project_path, project_data) = project();
        // What the effect loaded of the stored media, which still tells its footage when it's gone
        let loaded = self.gyroflow.as_ref().filter(|x| x.num_frames > 0 && x.fps > 0.0 && x.original_video_size.0 > 0 && x.original_video_size.1 > 0)
            .map(|x| Footage { size: x.original_video_size, fps: x.fps, duration_ms: x.num_frames as f64 * 1000.0 / x.fps });
        let change = self.stored.write().follow_media(host_media, &project_path, &project_data, loaded);
        if change == MediaChange::OtherClip {
            if let Some(base) = self.gyroflow.as_mut() {
                base.reload_values_from_project = true;
                base.keep_adjustments = true;
                // With an instance id of its own already
                base.ever_changed = true;
                base.managers.clear();
                // The stabilization of the clip it was on may read its keyframes from these (see `set_keyframe_provider`)
                let keyframable_params = base.keyframable_params.read().clone();
                base.keyframable_params = Arc::new(RwLock::new(keyframable_params));
            }
        }
        change
    }

    /// A copy was made of this instance, `id`, since it last had its lock: it shares its stabilization with the copy from
    /// now on (see `effect_instances`)
    fn sync_copies(&mut self, id: u64) {
        if effect_instances::take_copied(id) {
            if let Some(base) = self.gyroflow.as_mut() {
                base.ever_changed = false;
            }
        }
    }

    fn stab_manager(&mut self, params: &mut ParamHandler, global: &Plugin, output_rect: ae::Rect) -> Option<Arc<StabilizationManager>> {
        let out_size = (output_rect.width() as usize, output_rect.height() as usize);

        self.gyroflow.as_mut().unwrap().stab_manager(params, &global.gyroflow.manager_cache, out_size, false).ok()
    }

    fn smart_render(plugin: &PluginState, extra: SmartRenderExtra, is_gpu: bool) -> Result<(), ae::Error> {
        let in_data = plugin.in_data;
        let cb = extra.callbacks();
        let stab = extra.pre_render_data::<RenderData>();
        if stab.is_none() {
            log::error!("empty stab data in smart_render");
            return Ok(());
        }
        let Some(mut input_world) = cb.checkout_layer_pixels(0)? else {
            return Ok(());
        };
        if let Ok(Some(mut output_world)) = cb.checkout_output() {
            if let Ok(world_suite) = ae::pf::suites::World::new() {
                let pixel_format = world_suite.pixel_format(&input_world).unwrap();
                if is_gpu && pixel_format != ae::PixelFormat::GpuBgra128 {
                    log::info!("GPU render requested but pixel format is not GpuBgra128. It's: {:?}", pixel_format);
                    return Err(Error::UnrecogizedParameterType);
                }
                if let Some(stab) = stab {
                    let RenderData { stab, stored } = stab;
                    // log::info!("smart_render: timestamp: {} time: {}, time_step: {}, time_scale: {}, frame: {}, local_frame: {}",
                    //     in_data.current_timestamp(),
                    //     in_data.current_time(),
                    //     in_data.time_step(),
                    //     in_data.time_scale(),
                    //     in_data.current_frame(),
                    //     in_data.current_frame_local()
                    // );

                    let mut timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

                    let _ = (|| -> Result<(), ae::Error> {
                        let layer_flags = in_data.effect().layer()?.flags()?;

                        if layer_flags.contains(LayerFlags::TIME_REMAPPING) {
                            let plugin_id = unsafe { AEGP_PLUGIN_ID };
                            if let Ok(tr) = in_data.effect().layer()?.new_layer_stream(plugin_id, LayerStream::TimeRemap) {
                                let time = ae::Time { value: in_data.current_time(), scale: in_data.time_scale() };
                                if let Ok(StreamValue::OneD(v)) = tr.new_value(plugin_id, TimeMode::LayerTime, time, false) {
                                    timestamp_us = (v * 1_000_000.0).round() as i64;
                                }
                            }
                        }

                        if !layer_flags.contains(LayerFlags::FRAME_BLENDING) {
                            let fps = stab.params.read().fps;

                            let frame = timestamp_us as f64 * (fps / 1_000_000.0);
                            let frame = if frame.fract() > 0.999 { frame.ceil() } else { frame.floor() };
                            timestamp_us = (frame.floor() * (1_000_000.0 / fps)).round() as i64;
                        }
                        Ok(())
                    })();

                    let src_size = (input_world.width(), input_world.height(), input_world.buffer_stride());
                    let dest_size = (output_world.width(), output_world.height(), output_world.buffer_stride());
                    // let src_rect = GyroflowPluginBase::get_center_rect(input_world.width(),  input_world.height(), org_ratio);

                    let what_gpu = extra.what_gpu();
                    // log::info!("Render API: {what_gpu:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dest_size: {dest_size:?}");

                    let gpu_suite = ae::pf::suites::GPUDevice::new();

                    let buffers = if is_gpu && !gpu_suite.is_err() {
                        let gpu_suite = gpu_suite.unwrap(); // Safe because we checked for error above
                        let device_info = gpu_suite.device_info(in_data.effect_ref(), extra.device_index())?;

                        let in_ptr = gpu_suite.gpu_world_data(in_data.effect_ref(), input_world)?;
                        let out_ptr = gpu_suite.gpu_world_data(in_data.effect_ref(), output_world)?;

                        // log::info!("Render GPU: {in_ptr:?} -> {out_ptr:?}. API: {what_gpu:?}, pixel_format: {pixel_format:?}");

                        match what_gpu {
                            #[cfg(any(target_os = "windows", target_os = "linux"))]
                            ae::GpuFramework::Cuda => (
                                BufferSource::CUDABuffer { buffer: in_ptr },
                                BufferSource::CUDABuffer { buffer: out_ptr },
                                true
                            ),
                            #[cfg(any(target_os = "macos", target_os = "ios"))]
                            ae::GpuFramework::Metal => (
                                BufferSource::MetalBuffer { buffer: in_ptr  as *mut std::ffi::c_void, command_queue: device_info.command_queuePV as *mut std::ffi::c_void },
                                BufferSource::MetalBuffer { buffer: out_ptr as *mut std::ffi::c_void, command_queue: std::ptr::null_mut() },
                                true
                            ),
                            ae::GpuFramework::OpenCl => (
                                BufferSource::OpenCL { texture: in_ptr,  queue: device_info.command_queuePV },
                                BufferSource::OpenCL { texture: out_ptr, queue: std::ptr::null_mut() },
                                true
                            ),
                            _ => panic!("Invalid GPU framework")
                        }
                    } else {
                        (
                            BufferSource::Cpu { buffer: input_world.buffer_mut() },
                            BufferSource::Cpu { buffer: output_world.buffer_mut()},
                            false
                        )
                    };

                    // The layer is the clip at its own size, the right way up (see `GyroflowPluginBase::render_geometry`)
                    let input_rotation = {
                        let params = ParamHandler { inner: ParamsInner::AeRO(plugin.params), stored: stored.clone() };
                        let video_size = stab.params.read().size;
                        GyroflowPluginBase::render_geometry(video_size, params.get_f64(Params::InputRotation).unwrap_or_default(), (src_size.0, src_size.1), (dest_size.0, dest_size.1), true, false, false).kernel_rotation as f32
                    };

                    let mut buffers = Buffers {
                        input:  BufferDescription { size: src_size,  rect: None, data: buffers.0, rotation: Some(input_rotation), texture_copy: buffers.2 },
                        output: BufferDescription { size: dest_size, rect: None, data: buffers.1, rotation: None, texture_copy: buffers.2 }
                    };
                    if let Err(e) = match pixel_format {
                        ae::PixelFormat::GpuBgra128 |
                        ae::PixelFormat::Argb128    => stab.process_pixels::<RGBAf>(timestamp_us, None, &mut buffers),
                        ae::PixelFormat::Argb64     => stab.process_pixels::<RGBA16>(timestamp_us, None, &mut buffers),
                        ae::PixelFormat::Argb32     => stab.process_pixels::<RGBA8>(timestamp_us, None, &mut buffers),
                        _ => Err(GyroflowCoreError::UnsupportedFormat(format!("{pixel_format:?}")))
                    } {
                        log::error!("Failed to process pixels: {e:?}");
                    }
                } else {
                    output_world.copy_from(&input_world, None, None)?;
                }
            }
        }
        cb.checkin_layer_pixels(0)?;

        Ok(())
    }

    fn cpu_render(plugin: &PluginState, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        let in_data = plugin.in_data;
        if let Some(stab) = in_data.frame_data::<RenderData>() {
            let RenderData { stab, stored } = stab;
            let timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

            let src_size = (src.width() as usize, src.height() as usize, src.buffer_stride());
            let dst_size = (dst.width() as usize, dst.height() as usize, dst.buffer_stride());

            // The layer is the clip at its own size, the right way up (see `GyroflowPluginBase::render_geometry`)
            let geometry = {
                let params = ParamHandler { inner: ParamsInner::AeRO(plugin.params), stored: stored.clone() };
                let video_size = stab.params.read().size;
                GyroflowPluginBase::render_geometry(video_size, params.get_f64(Params::InputRotation).unwrap_or_default(), (src_size.0, src_size.1), (dst_size.0, dst_size.1), true, false, false)
            };
            let src_rect = geometry.src_rect;

            // log::info!("org_ratio: {org_ratio:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dst_size: {dst_size:?}, src.stride: {}, bit_depth: {}", src.row_bytes(), src.bit_depth());

            let src_buffer = unsafe { std::slice::from_raw_parts_mut(src.buffer().as_ptr() as *mut u8, src.buffer().len()) };
            let dst_buffer = unsafe { std::slice::from_raw_parts_mut(dst.buffer().as_ptr() as *mut u8, dst.buffer().len()) };

            let mut buffers = Buffers {
                input:  BufferDescription { size: src_size, rect: Some(src_rect), data: BufferSource::Cpu { buffer: src_buffer }, rotation: Some(geometry.kernel_rotation as f32), texture_copy: false },
                output: BufferDescription { size: dst_size, rect: None,           data: BufferSource::Cpu { buffer: dst_buffer }, rotation: None, texture_copy: false }
            };
            if let Err(e) = match src.bit_depth() {
                8  => stab.process_pixels::<RGBA8> (timestamp_us, None, &mut buffers),
                16 => stab.process_pixels::<RGBA16>(timestamp_us, None, &mut buffers),
                32 => stab.process_pixels::<RGBAf> (timestamp_us, None, &mut buffers),
                bd => panic!("Unknown bit depth: {bd}")
            } {
                log::error!("Failed to process pixels: {e:?}");
            }
        } else {
            dst.copy_from(src, None, None)?;
        }

        Ok(())
    }
}

impl AdobePluginGlobal for Plugin {
    fn params_setup(&self, params: &mut ae::Parameters<Params>, in_data: InData, _: OutData) -> Result<(), Error> {
        // Logo
        params.add_customized(Params::Logo, "", ae::NullDef::new(), |param| {
            param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
            param.set_ui_flags(ae::ParamUIFlags::CONTROL);
            param.set_ui_width(250);
            param.set_ui_height(60);
            -1
        })?;

        for param in GyroflowPluginBase::get_param_definitions() {
            define_param(params, param, None);
        }

        // Save in global memory for use in Premiere GPU Filter entry
        param_index_for_type(Params::Fov, Some((*params.map).clone()));

        in_data.interact().register_ui(
            CustomUIInfo::new()
                .events(ae::CustomEventFlags::EFFECT)
        )?;

        Ok(())
    }

    fn handle_command(&self, cmd: ae::Command, in_data: InData, mut out_data: OutData, _params: &mut ae::Parameters<Params>) -> Result<(), ae::Error> {
        self.gyroflow.initialize_log("adobe");

        // log::info!("global command: {:?}, thread: {:?}, ptr: {:?}, effect_ref: {:?}", cmd, std::thread::current().id(), self as *const _, in_data.effect_ref().as_ptr());

        match cmd {
            ae::Command::About => {
                out_data.set_return_msg(concat!("Gyroflow, v", env!("CARGO_PKG_VERSION"), "\nCopyright 2024 AdrianEddy\rGyroflow plugin."));
            }
            ae::Command::GlobalSetup => {
                gyroflow_core::gpu::initialize_contexts();

                if in_data.is_premiere() {
                    unsafe {
                        IS_PREMIERE = true;
                        GLOBAL_INST.set(self as *const _ as usize).unwrap();
                    }
                    use ae::pr::PixelFormat::*;
                    let pixel_format = ae::pf::suites::PixelFormat::new()?;
                    pixel_format.clear_supported_pixel_formats(in_data.effect_ref())?;
                    let supported_formats = [
                        Bgra4444_8u,  Vuya4444_8u,  Vuya4444_8u709,  Argb4444_8u,  Bgrx4444_8u,  Vuyx4444_8u,  Vuyx4444_8u709,  Xrgb4444_8u,  Bgrp4444_8u,  Vuyp4444_8u,  Vuyp4444_8u709,  Prgb4444_8u,
                        Bgra4444_16u, Vuya4444_16u,                  Argb4444_16u, Bgrx4444_16u,                                Xrgb4444_16u, Bgrp4444_16u,                                Prgb4444_16u,
                        Bgra4444_32f, Vuya4444_32f, Vuya4444_32f709, Argb4444_32f, Bgrx4444_32f, Vuyx4444_32f, Vuyx4444_32f709, Xrgb4444_32f, Bgrp4444_32f, Vuyp4444_32f, Vuyp4444_32f709, Prgb4444_32f,
                        Bgra4444_32fLinear, Bgrp4444_32fLinear, Bgrx4444_32fLinear, Argb4444_32fLinear, Prgb4444_32fLinear, Xrgb4444_32fLinear
                    ];
                    for x in supported_formats {
                        pixel_format.add_supported_pixel_format(in_data.effect_ref(), x)?;
                    }
                    out_data.set_out_flag2(ae::OutFlags2::SupportsGpuRenderF32, false);
                } else {
                    out_data.set_out_flag2(ae::OutFlags2::SupportsGpuRenderF32, true);
                    let _ = UI_THREAD.set(std::thread::current().id());

                    if let Ok(util) = ae::aegp::suites::Utility::new() {
                        unsafe { AEGP_PLUGIN_ID = util.register_with_aegp("Gyroflow")?; }
                    }
                }

                let _ = in_data.effect().set_options_button_name("Open Gyroflow");
            },
            ae::Command::ArbitraryCallback { extra } => {
                // All the arbitrary data parameters are strings
                if let Err(e) = ArbString::callback(&extra) {
                    log::info!("arb callback error, which: {:?}", extra.which_function());
                    return Err(e);
                }
            }
            ae::Command::GlobalSetdown => {
                self.gyroflow.manager_cache.lock().clear();
            }
            ae::Command::GpuDeviceSetup { extra } => {
                gyroflow_plugin_base::wgpu::WgpuWrapper::list_devices();
                gyroflow_core::gpu::initialize_contexts();
                let device_info = ae::pf::suites::GPUDevice::new().unwrap().device_info(in_data.effect_ref(), extra.device_index())?;

                let what_gpu = extra.what_gpu();

                log::info!("Device info: {device_info:?}. GPU: {what_gpu:?}");

                if what_gpu != ae::GpuFramework::None {
                    out_data.set_out_flag2(ae::OutFlags2::SupportsGpuRenderF32, true);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
impl CrossThreadInstance {
    fn user_changed_param(&mut self, plugin: &mut PluginState, param: Params) -> Result<(), ae::Error> {
        // The output size the user sets, rather than the sequence's. And a stabilization of the effect's own from now on,
        // when it changes it, rather than the one of the effects it was copied with (see `param_changed`). The instance is
        // the effect's own already (see `effect_instances`)
        if matches!(param, Params::OutputHeight | Params::OutputWidth | Params::OutputSizeSwap | Params::OutputSizeToTimeline) {
            self.get().unwrap().read().stored.write().sequence_size = (0, 0);
        }
        {
            let stored = {
                let _self = self.get().unwrap();
                let mut _self = _self.read();
                _self.stored.clone()
            };
            let (pending_f64s, pending_bools, pending_strings, pending_i32s) = {
                let mut stored = stored.write();
                stored.pending_params_f64.remove(&param);
                stored.pending_params_bool.remove(&param);
                stored.pending_params_str.remove(&param);
                stored.pending_params_i32.remove(&param);

                (
                    stored.pending_params_f64.keys().copied().collect::<Vec<_>>(),
                    stored.pending_params_bool.keys().copied().collect::<Vec<_>>(),
                    stored.pending_params_str.keys().copied().collect::<Vec<_>>(),
                    stored.pending_params_i32.keys().copied().collect::<Vec<_>>()
                )
            };
            // As they're pending when set: a render thread can change them meanwhile (the effect moved to another clip)
            let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: stored.clone() };
            for k in pending_f64s    { let v = stored.read().pending_params_f64.get(&k).copied();  if let Some(v) = v { let _ = params.set_f64(k, v); } }
            for k in pending_bools   { let v = stored.read().pending_params_bool.get(&k).copied(); if let Some(v) = v { let _ = params.set_bool(k, v); } }
            for k in pending_strings { let v = stored.read().pending_params_str.get(&k).cloned();  if let Some(v) = v { let _ = params.set_string(k, &v); } }
            for k in pending_i32s    { let v = stored.read().pending_params_i32.get(&k).copied();  if let Some(v) = v { let _ = params.set_i32(k, v); } }
        }

        let _self = self.get().unwrap();

        let mut _self = _self.write();
        _self.sync_copies(self.id);

        let mut fps = _self.gyroflow.as_ref().unwrap().fps;
        if fps == 0.0 {
            fps = _self.stored.read().media_fps;
        } else {
            _self.stored.write().media_fps = fps;
        }

        let stored = _self.stored.clone();
        let stored2 = _self.stored.clone();
        if let Some(inst) = _self.gyroflow.as_mut() {
            if param == Params::OutputSizeToTimeline && plugin.in_data.is_after_effects() {
                let _ = (|| -> Result<(), ae::Error> {
                    let comp_size = plugin.in_data.effect()
                                                  .layer()?
                                                  .parent_comp()?
                                                  .item()?
                                                  .dimensions()?;
                    inst.timeline_size = (comp_size.0 as _, comp_size.1 as _);
                    Ok(())
                })();
            }

            let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: stored };

            if fps > 0.0 && plugin.in_data.is_after_effects() {
                let mut speed_per_frame = Vec::new();
                if params.get_bool(Params::StabilizationSpeedRamp).unwrap_or_default() {
                    let _ = (|| -> Result<(), ae::Error> {
                        let layer = plugin.in_data.effect().layer()?;
                        if layer.flags()?.contains(LayerFlags::TIME_REMAPPING) {
                            let plugin_id = unsafe { AEGP_PLUGIN_ID };
                            if let Ok(tr) = layer.new_layer_stream(plugin_id, LayerStream::TimeRemap) {
                                let mut prev_original_ts = 0.0;
                                let mut prev_new_ts = 0.0;
                                let mut frame = 0;

                                loop {
                                    let original_ts = frame as f64 / fps;
                                    let time = ae::Time { value: (original_ts * plugin.in_data.time_scale() as f64).round() as i32, scale: plugin.in_data.time_scale() };
                                    if let Ok(StreamValue::OneD(new_ts)) = tr.new_value(plugin_id, TimeMode::LayerTime, time, false) {
                                        if frame > 0 {
                                            let original_diff = original_ts - prev_original_ts;
                                            let new_diff = new_ts - prev_new_ts;
                                            let speed = (new_diff / original_diff) * 100.0;
                                            if speed.abs() > 0.001 {
                                                speed_per_frame.push(speed);
                                            } else {
                                                break;
                                            }
                                        } else {
                                            speed_per_frame.push(100.0);
                                        }
                                        prev_original_ts = original_ts;
                                        prev_new_ts = new_ts;
                                    }
                                    frame += 1;
                                }
                            }
                        }
                        Ok(())
                    })();
                }
                if stored2.read().speed_per_frame != speed_per_frame {
                    stored2.write().speed_per_frame = speed_per_frame;
                }
            }

            //let current_instance_id = params.get_string(Params::InstanceId).unwrap_or_default();
            let handled = inst.param_changed(&mut params, &plugin.global.gyroflow.manager_cache, param, true).map_err(|e| log::error!("param_changed error: {e:?}")).is_ok();
            // The changes not reported before this edit. And this one's state taken, for the render not to cache it again
            update_keyframes(inst, &mut params, &plugin.global.gyroflow.manager_cache, handled.then_some(param));

            /*if current_instance_id != params.get_string(Params::InstanceId).unwrap_or_default() {
                log::warn!("Instance ID changed, creating new cross thread instance!");
                self.id = fastrand::u64(..);
            }*/

            if let Ok(stab) = inst.stab_manager(&mut params, &plugin.global.gyroflow.manager_cache, (0, 0), false) {
                if plugin.in_data.is_after_effects() && param == Params::CreateCamera {
                    for (org_cam, smooth_cam, name) in [(true, false, "Original camera"), (false, true, "Smoothed camera")] {
                        let fields = format!("{{ \"original\": {{ \"euler_angles\": {org_cam} }}, \"stabilized\": {{ \"euler_angles\": {smooth_cam} }}, \"zooming\": {{ }} }}");
                        let script = gyroflow_core::gyro_export::export_gyro_data("camera.jsx", &fields, &stab);

                        let plugin_id = unsafe { AEGP_PLUGIN_ID };
                        let comp = plugin.in_data.effect().layer()?.parent_comp()?;
                        let _ = (|| -> Option<()> {
                            let data: serde_json::Value = serde_json::from_str(&script.split(";").next()?.replace("var data = ", "")).ok()?;

                            let time_scale = plugin.in_data.time_scale();
                            let frame_times = data.get("frame_times")?.as_array()?.into_iter().filter_map(|x| {
                                Some(Time { value: (x.as_f64()? * time_scale as f64).round() as _, scale: time_scale })
                            }).collect::<Vec<Time>>();

                            let orientations = data.get("orientations")?.as_array()?;
                            let cam = comp.create_camera(name, ae::FloatPoint { x: 0.0, y: 0.0 }.into()).ok()?;
                            if org_cam {
                                cam.set_flag(ae::aegp::LayerFlags::VIDEO_ACTIVE, false).ok()?;
                            }
                            if let Ok(o) = cam.new_layer_stream(plugin_id, LayerStream::Orientation) {
                                let kf = o.keyframes().ok()?;
                                let mut kfs = kf.start_add_keyframes().ok()?;
                                for (i, time) in frame_times.iter().enumerate() {
                                    let value = orientations.get(i)?.as_array()?;

                                    let ind = kfs.add_keyframes(ae::aegp::TimeMode::LayerTime, *time).ok()?;
                                    kfs.set_add_keyframe(ind, o.as_ptr(), StreamValue::ThreeD { x: value.get(0)?.as_f64()?, y: value.get(1)?.as_f64()?, z: value.get(2)?.as_f64()? }).ok()?;
                                }
                            }
                            if let Ok(o) = cam.new_layer_stream(plugin_id, LayerStream::Zoom) {
                                let kf = o.keyframes().ok()?;
                                let mut kfs = kf.start_add_keyframes().ok()?;
                                if let Some(zooms) = data.get("zooms").and_then(|x| x.as_array()) {
                                    for (i, time) in frame_times.iter().enumerate() {
                                        let ind = kfs.add_keyframes(ae::aegp::TimeMode::LayerTime, *time).ok()?;
                                        kfs.set_add_keyframe(ind, o.as_ptr(), StreamValue::OneD(zooms.get(i)?.as_f64()?)).ok()?;
                                    }
                                } else if let Some(zoom) = data.get("zoom").and_then(|x| x.as_f64()) {
                                    let ind = kfs.add_keyframes(ae::aegp::TimeMode::LayerTime, Time { value: plugin.in_data.current_time(), scale: time_scale }).ok()?;
                                    kfs.set_add_keyframe(ind, o.as_ptr(), StreamValue::OneD(zoom)).ok()?;
                                }
                            }
                            Some(())
                        })();
                    }
                }
            }
        }

        Ok(())
    }
}

impl AdobePluginInstance for CrossThreadInstance {
    fn flatten(&self) -> Result<(u16, Vec<u8>), Error> {
        // Saved sharing its stabilization with the copies made of it meanwhile, before it rendered again
        if let Some(inst) = self.get() {
            inst.write().sync_copies(self.id);
        }
        Ok((sequence_data::VERSION, bincode::serde::encode_to_vec(&self, bincode::config::legacy()).unwrap()))
    }
    fn unflatten(version: u16, bytes: &[u8]) -> Result<Self, Error> {
        // For the `SequenceResetup` that follows: what's restored for the effect may be a copy of another effect's
        effect_instances::set_restored(version, bytes);
        Ok(Self::restore(version, bytes))
    }

    fn do_dialog(&mut self, plugin: &mut PluginState) -> Result<(), ae::Error> {
        self.user_changed_param(plugin, Params::OpenGyroflow)
    }

    fn render(&self, plugin: &mut PluginState, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        Instance::cpu_render(plugin, src, dst)
    }

    fn handle_command(&mut self, plugin: &mut PluginState, cmd: ae::Command) -> Result<(), ae::Error> {
        // log::info!("sequence command: {:?}, thread: {:?}, ptr: {:?}", cmd, std::thread::current().id(), self as *const _);

        let restored = effect_instances::take_restored(self.id).filter(|_| matches!(cmd, ae::Command::SequenceResetup));
        let (in_flags, sequence_data) = unsafe { ((*plugin.in_data.as_ptr()).in_flags, (*plugin.in_data.as_ptr()).sequence_data) };
        // After Effects sets up the sequence data for render threads in their copy of the project. Its effects' own, it sets
        // up and disposes of on its UI thread
        let render_only = in_flags & ae::sys::PF_InFlag_PROJECT_IS_RENDER_ONLY as ae::sys::PF_InFlags != 0;
        let ui_thread = UI_THREAD.get().is_some_and(|x| *x == std::thread::current().id());
        if matches!(cmd, ae::Command::SequenceFlatten | ae::Command::SequenceSetdown) && !render_only && ui_thread {
            effect_instances::release(self.id);
        }
        // The sequence data disposed of may be flat (an effect pasted onto itself in After Effects)
        let key = EffectKey::of(&plugin.in_data).filter(|_| !matches!(cmd, ae::Command::SequenceSetdown));
        let Some(key) = key else {
            return self.handle_instance_command(plugin, cmd, false);
        };
        // The effect's own instance (see `effect_instances`). Set up with sequence data copied from another effect, it gets
        // a copy, and a render thread's sequence data may be older than the effect's
        // After Effects gives render threads the sequence data read only
        let read_only = sequence_data.is_null() && !matches!(cmd, ae::Command::SequenceSetup);
        let render_only = matches!(cmd, ae::Command::SequenceResetup) && render_only;
        let setup = matches!(cmd, ae::Command::SequenceSetup | ae::Command::SequenceResetup) && !render_only && ui_thread;
        let (mut inst, own) = if (read_only || render_only) && matches!(key, EffectKey::Layer(..)) {
            self.found_for(key).map(|x| (x, true)).unwrap_or((Self { id: self.id }, false))
        } else {
            // Restored for the effect, rather than for a render clone. A `SequenceSetup`'s is a new instance, which the
            // effect only gets when it has none (Premiere is reported to send it to render clones too)
            (self.of_effect(key, !render_only && restored.is_some(), restored.as_ref().map(|(version, bytes)| (*version, &bytes[..])), setup), true)
        };
        if matches!(cmd, ae::Command::SequenceSetup) && inst.id == self.id {
            // A new effect: its instance id is its own, and so is its stabilization (see `ever_changed`)
            if let Some(base) = inst.get().unwrap().write().gyroflow.as_mut() {
                base.ever_changed = true;
            }
        }
        if !read_only {
            self.id = inst.id;
        }
        inst.handle_instance_command(plugin, cmd, own)
    }
}

impl CrossThreadInstance {
    /// `own`: whether this is the effect's own instance, which it can change for the clip it's on
    fn handle_instance_command(&mut self, plugin: &mut PluginState, cmd: ae::Command, own: bool) -> Result<(), ae::Error> {
        let in_data = &mut plugin.in_data;

        match cmd {
            ae::Command::UserChangedParam { param_index } => {
                self.user_changed_param(plugin, plugin.params.type_at(param_index))?;
                plugin.out_data.set_force_rerender();
            }
            ae::Command::SequenceSetup => {
                let _self = self.get().unwrap();
                let _self = _self.read();
                let mut stored = _self.stored.write();

                let layer = in_data.effect().layer();
                let clip = layer.as_ref().ok().and_then(|x| LayerClip::of(x, "").ok()).unwrap_or_default();
                let sequence_size = layer.and_then(|x| x.parent_comp()?.item()?.dimensions()).map(|x| (x.0 as usize, x.1 as usize));
                stored.on_clip(sequence_size.unwrap_or((in_data.width() as _, in_data.height() as _)), clip.anamorphic(), true);
                let footage_path = clip.media;

                if !footage_path.is_empty() {
                    stored.pending_params_str.insert(Params::ProjectPath, GyroflowPluginBase::get_project_path(&footage_path).unwrap_or(footage_path.to_owned()));
                    stored.media_file_path = footage_path;
                } else if in_data.is_after_effects() {
                    plugin.out_data.set_return_msg("Unable to find the footage path.\nUse the \"Browse\" button and load the project file or a video file.");
                }
            }
            ae::Command::SmartPreRender { mut extra } => {
                let what_gpu = extra.what_gpu();
                let mut req = extra.output_request();

                // We always need to request the full input frame
                req.rect = ae::sys::PF_LRect { left: 0, top: 0, right: in_data.width(), bottom: in_data.height() };

                if what_gpu != ae::GpuFramework::None {
                    extra.set_gpu_render_possible(true);
                }

                let cb = extra.callbacks();
                if let Ok(in_result) = cb.checkout_layer(0, 0, &req, in_data.current_time(), in_data.time_step(), in_data.time_scale()) {
                    let     _result_rect = extra.union_result_rect(in_result.result_rect.into());
                    let _max_result_rect = extra.union_max_result_rect(in_result.max_result_rect.into());

                    let _self = self.get().unwrap();
                    let mut _self = _self.write();
                    _self.sync_copies(self.id);
                    let stored = _self.stored.clone();

                    let layer = in_data.effect().layer();
                    let stored_media = stored.read().media_file_path.clone();
                    let clip = layer.as_ref().ok().and_then(|x| LayerClip::of(x, &stored_media).ok()).unwrap_or_default();

                    let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: _self.stored.clone() };

                    // Relinked footage, at another path than the one stored when the effect was applied, or the effect copied
                    // to another layer. Not while it's offline, or when it can't be told in a precomp
                    if own && !clip.missing && !clip.ambiguous {
                        let change = _self.follow_media(&clip.media, || (params.get_string(Params::ProjectPath).unwrap_or_default(), params.get_string(Params::ProjectData).unwrap_or_default()));
                        if change == MediaChange::OtherClip {
                            let sequence_size = layer.and_then(|x| x.parent_comp()?.item()?.dimensions()).map(|x| (x.0 as usize, x.1 as usize));
                            stored.write().on_clip(sequence_size.unwrap_or((in_data.width() as _, in_data.height() as _)), clip.anamorphic(), false);
                        }
                    }

                    let (sx, sy) = (f64::from(in_data.downsample_x()), f64::from(in_data.downsample_y()));

                    let (w, h) = (params.get_f64(Params::OutputWidth).unwrap(), params.get_f64(Params::OutputHeight).unwrap());
                    let (x, y) = ((in_result.ref_width as f64 - w) / 2.0, (in_result.ref_height as f64 - h) / 2.0);

                    extra.set_result_rect(ae::Rect { left: (x * sx).round() as _, top: (y * sy).round() as _, right: ((x + w) * sx).round() as _, bottom: ((y + h) * sy).round() as _ });
                    extra.set_max_result_rect(extra.result_rect());
                    extra.set_returns_extra_pixels(true);

                    let full_rect = ae::Rect { left: 0, top: 0, right: w as _, bottom: h as _ };

                    if let Some(base) = _self.gyroflow.as_mut() {
                        update_keyframes(base, &mut params, &plugin.global.gyroflow.manager_cache, None);
                    }

                    if let Some(mut stab) = _self.stab_manager(&mut params, plugin.global, full_rect) {
                        let duration_ms = stab.params.read().duration_ms;
                        let ms = |ranges: &[(f64, f64)]| ranges.iter().map(|x| ((x.0 * duration_ms).round() as i64, (x.1 * duration_ms).round() as i64)).collect::<Vec<_>>();
                        let old_ranges = Some(stab.trim_ranges()).filter(|x| !x.is_empty()).unwrap_or(vec![(0.0, 1.0)]);
                        let new_range = clip.trim.unwrap_or((0.0f64, duration_ms / 1000.0));
                        let new_ranges = vec![((new_range.0 * 1000.0) / duration_ms, (new_range.1 * 1000.0) / duration_ms)];
                        if ms(&old_ranges) != ms(&new_ranges) {
                            log::info!("Trim range changed: {:?} != {:?}", ms(&old_ranges), ms(&new_ranges));
                            // The pieces of a cut clip are trimmed each their own way, and smoothed for it: a stabilization shared
                            // with the effect's copies stays as it is for them (see `ever_changed`)
                            let own_stab = match _self.gyroflow.as_ref().map(|x| x.ever_changed) {
                                Some(false) => {
                                    let detached = _self.gyroflow.as_mut().unwrap().detach(&mut params, &plugin.global.gyroflow.manager_cache).map_err(|e| log::error!("detach error: {e:?}")).is_ok();
                                    match _self.stab_manager(&mut params, plugin.global, full_rect).filter(|_| detached) {
                                        Some(forked) => { stab = forked; true }
                                        None => false
                                    }
                                }
                                _ => true
                            };
                            if own_stab {
                                stab.set_trim_ranges(new_ranges);
                                stab.invalidate_blocking_smoothing();
                            }
                        }
                        extra.set_pre_render_data::<RenderData>(RenderData { stab, stored });
                    } else {
                        extra.set_result_rect(ae::Rect::empty());
                        extra.set_max_result_rect(extra.result_rect());
                        return Err(ae::Error::InvalidParms);
                    }
                }
            }
            ae::Command::FrameSetup { out_layer, .. } => {
                let _self = self.get().unwrap();
                let mut _self = _self.write();
                _self.sync_copies(self.id);
                let stored = _self.stored.clone();

                let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: _self.stored.clone() };

                if params.get_string(Params::ProjectPath).unwrap().is_empty() {
                    return Ok(());
                }

                // Output buffer resizing may only occur during FrameSetup.
                let (sx, sy) = (f64::from(in_data.downsample_x()), f64::from(in_data.downsample_y()));
                let (nw, nh) = ((params.get_f64(Params::OutputWidth).unwrap() * sx).round() as u32, (params.get_f64(Params::OutputHeight).unwrap() * sy).round() as u32);
                plugin.out_data.set_width(nw as _);
                plugin.out_data.set_height(nh as _);

                if let Some(stab) = _self.stab_manager(&mut params, plugin.global, out_layer.extent_hint()) {
                    plugin.out_data.set_frame_data::<RenderData>(RenderData { stab, stored })
                } else {
                    log::error!("frame_setup: no stab manager");
                }
            }
            ae::Command::FrameSetdown => {
                in_data.destroy_frame_data::<RenderData>();
            }
            ae::Command::SmartRender { extra } => {
                Instance::smart_render(&plugin, extra, false)?;
            }
            ae::Command::SmartRenderGpu { extra } => {
                Instance::smart_render(&plugin, extra, true)?;
            }
            ae::Command::Event { mut extra } => {
                match extra.event() {
                    ae::Event::Draw(_)  => { ui::draw(&in_data, plugin.params, &mut extra, self)?; }
                    _ => {}
                }
            }
            _ => { }
        }

        Ok(())
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        CrossThreadInstance::clear_map();
        log::info!("dropping plugin: {:?}", self as *const _);
        {
            let mut lock = self.gyroflow.manager_cache.lock();
            for (_, v) in lock.iter() {
                log::info!("arc count: {}", Arc::strong_count(v));
            }
            lock.clear();
        }
        log::info!("dropped plugin: {:?}", self as *const _);
    }
}

ae::define_effect!(Plugin, CrossThreadInstance, Params);

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder with files in it, deleted when dropped
    struct Folder(std::path::PathBuf);
    impl Folder {
        fn join(&self, file: &str) -> std::path::PathBuf { self.0.join(file) }
        fn path(&self, file: &str) -> String { self.join(file).to_string_lossy().to_string() }
    }
    impl Drop for Folder {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    /// A folder with `files` in it, each with its name in it, and the path of one that doesn't exist
    fn folder(files: &[&str]) -> (Folder, String) {
        let dir = std::env::temp_dir().join(format!("gyroflow-adobe-test-{}", fastrand::u64(..)));
        for f in files {
            let path = dir.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, f.rsplit('/').next().unwrap()).unwrap();
        }
        (Folder(dir), std::env::temp_dir().join("gyroflow-adobe-test-gone").join("C0001.MP4").to_string_lossy().to_string())
    }
    fn stored(media: &str, project: &str) -> StoredParams {
        let mut stored = StoredParams::default();
        stored.media_file_path = media.to_owned();
        stored.pending_params_str.insert(Params::ProjectPath, project.to_owned());
        stored
    }
    fn project_path(stored: &StoredParams) -> &str {
        stored.pending_params_str.get(&Params::ProjectPath).unwrap()
    }
    /// What `follow_media` finds, with the project as stored
    fn follow(s: &mut StoredParams, media: &str) -> MediaChange {
        let (project_path, project_data) = (project_path(s).to_owned(), s.pending_params_str.get(&Params::ProjectData).cloned().unwrap_or_default());
        s.follow_media(media, &project_path, &project_data, None)
    }

    #[test]
    fn follows_relinked_media() {
        let (dir, gone) = folder(&["C0001.MP4"]);
        let media = dir.path("C0001.MP4");

        let mut s = stored(&gone, &gone);
        assert_eq!(follow(&mut s, &media), MediaChange::Moved);
        assert_eq!(s.media_file_path, media);
        assert_eq!(project_path(&s), media);
        // Followed once
        assert_eq!(follow(&mut s, &media), MediaChange::None);
    }

    #[test]
    fn follows_to_the_project_next_to_the_media() {
        let (dir, gone) = folder(&["C0001.MP4", "C0001.gyroflow"]);
        let media = dir.path("C0001.MP4");
        let gone_project = std::path::Path::new(&gone).with_extension("gyroflow").to_string_lossy().to_string();

        let mut s = stored(&gone, &gone_project);
        assert_eq!(follow(&mut s, &media), MediaChange::Moved);
        assert_eq!(project_path(&s), dir.path("C0001.gyroflow"));
    }

    #[test]
    fn keeps_the_project_that_still_loads_or_was_picked() {
        let (dir, gone) = folder(&["C0001.MP4", "renamed.MP4"]);
        let media = dir.path("C0001.MP4");

        // Picked by the user, somewhere else
        let picked = std::env::temp_dir().join("gyroflow-adobe-test-gone").join("other.gyroflow").to_string_lossy().to_string();
        let mut s = stored(&gone, &picked);
        assert_eq!(follow(&mut s, &media), MediaChange::Moved);
        assert_eq!(s.media_file_path, media);
        assert_eq!(project_path(&s), picked);

        // Loading from the embedded project data
        let gone_project = std::path::Path::new(&gone).with_extension("gyroflow").to_string_lossy().to_string();
        let mut s = stored(&gone, &gone_project);
        s.pending_params_str.insert(Params::ProjectData, "{}".into());
        assert_eq!(follow(&mut s, &media), MediaChange::Moved);
        assert_eq!(project_path(&s), gone_project);

        // Renamed as well, with nothing that tells it's the same footage: it starts over on it, as on another clip's
        let mut s = stored(&gone, &gone_project);
        s.pending_params_str.insert(Params::ProjectData, "{}".into());
        let instance_id = s.instance_id.clone();
        assert_eq!(follow(&mut s, &dir.path("renamed.MP4")), MediaChange::OtherClip);
        assert_ne!(s.instance_id, instance_id);
        assert_eq!(project_path(&s), dir.path("renamed.MP4"));
    }

    #[test]
    fn waits_for_media_offline_in_the_host() {
        let (dir, gone) = folder(&["C0001.MP4"]);
        let a = dir.path("C0001.MP4");
        let offline = std::path::Path::new(&gone).with_file_name("C0002.MP4").to_string_lossy().to_string();

        for stored_media in [a.clone(), gone.clone()] {
            let mut s = stored(&stored_media, &stored_media);
            let instance_id = s.instance_id.clone();
            assert_eq!(follow(&mut s, &offline), MediaChange::None);
            assert_eq!(s.media_file_path, stored_media);
            assert_eq!(s.instance_id, instance_id);
        }
    }

    #[test]
    fn follows_a_copy_of_the_media() {
        let (dir, _) = folder(&["A/C0001.MP4", "B/C0001.MP4", "B/C0002.MP4"]);
        let a = dir.path("A/C0001.MP4");
        std::fs::write(dir.join("B/C0002.MP4"), "C0001.MP4").unwrap();

        // Relinked to the copy on another card, with the original still there, renamed or not
        for media in [dir.path("B/C0001.MP4"), dir.path("B/C0002.MP4")] {
            let mut s = stored(&a, &a);
            assert_eq!(follow(&mut s, &media), MediaChange::Moved);
            assert_eq!(s.media_file_path, media);
            assert_eq!(project_path(&s), a);
        }
    }

    #[test]
    fn media_that_still_exists_is_another_clip() {
        let (dir, _) = folder(&["C0001.MP4", "C0002.MP4", "B/C0001.MP4"]);
        let a = dir.path("C0001.MP4");
        std::fs::write(dir.join("B/C0001.MP4"), "another C0001.MP4").unwrap();

        let mut s = stored(&a, &a);
        assert_eq!(follow(&mut s, ""), MediaChange::None);
        assert_eq!(follow(&mut s, &a), MediaChange::None);
        assert_eq!(s.media_file_path, a);
        // Named the same on another card, but another clip
        for media in [dir.path("C0002.MP4"), dir.path("B/C0001.MP4")] {
            let mut s = stored(&a, &a);
            assert_eq!(follow(&mut s, &media), MediaChange::OtherClip);
            assert_eq!(project_path(&s), media);
        }
    }

    #[test]
    fn relinked_by_the_footage_the_project_was_made_for() {
        let r = |stored_exists, same_name, for_host, for_stored| Relink { stored_exists, same_name, named_after: same_name, for_host, for_stored, ..Default::default() };
        for same_name in [false, true] {
            // A copy of the stored file
            assert!(relinked(Relink { copy_of_stored: true, ..r(true, same_name, None, None) }));
            // The project picked for the clip's media, the stored one another clip's (an effect saved by an older build)
            assert!(relinked(r(true, same_name, Some(true), Some(false))));
            assert!(relinked(r(true, same_name, Some(true), None)));
            // Made for both: another clip as long, named the same on another card or not
            assert!(!relinked(r(true, same_name, Some(true), Some(true))));
            assert!(!relinked(Relink { same_footage: Some(true), ..r(true, same_name, Some(true), Some(true)) }));
            // The stored file is still there: another clip, named the same on another card or not
            assert!(!relinked(r(true, same_name, None, None)));
            assert!(!relinked(r(true, same_name, Some(false), Some(true))));
            // Gone, with the footage of the project, or of what the effect loaded of it
            assert!(relinked(r(false, same_name, Some(true), None)));
            assert!(relinked(Relink { same_footage: Some(true), ..r(false, same_name, None, None) }));
        }
        // A transcode, named after it, the stored file there or not, the project made for both or of no known footage
        for stored_exists in [true, false] {
            assert!(relinked(Relink { named_after: true, same_footage: Some(true), ..r(stored_exists, false, Some(true), Some(true)) }));
            let transcode = Relink { named_after: true, same_footage: Some(true), ..r(stored_exists, false, None, None) };
            assert!(relinked(transcode));
            assert!(!relinked(Relink { same_footage: Some(false), ..transcode }));
            if stored_exists {
                assert!(!relinked(Relink { named_after: false, ..transcode }));
            }
        }
        // Gone and named the same: moved, the footage the app read doesn't overrule it
        assert!(relinked(r(false, true, None, None)));
        assert!(relinked(r(false, true, Some(false), None)));
        // Gone and renamed: of other footage (a preset made on another computer), or nothing tells
        assert!(!relinked(Relink { project_known: true, ..r(false, false, Some(false), None) }));
        assert!(!relinked(Relink { same_footage: Some(true), project_known: true, ..r(false, false, Some(false), None) }));
        assert!(!relinked(Relink { same_footage: Some(false), ..r(false, false, None, None) }));
        assert!(!relinked(r(false, false, None, None)));
        // Gone and renamed, of footage that can't be read (R3D...): the project that loads stays
        assert!(relinked(Relink { project_known: true, ..r(false, false, None, None) }));
    }

    #[test]
    fn transcodes_are_named_after_the_media() {
        assert!(is_named_after("E:/Proxies/C0001.mov", "D:/CLIP/C0001.MP4"));
        assert!(is_named_after("E:/Proxies/C0001_Proxy.mp4", "D:/CLIP/C0001.MP4"));
        assert!(is_named_after("E:\\Transcoded\\C0001 (ProRes).mov", "D:\\CLIP\\C0001.MP4"));
        assert!(is_named_after("D:/CLIP/C0001.MP4", "E:/CLIP/C0001.MP4"));
        assert!(!is_named_after("D:/CLIP/C00010.MP4", "D:/CLIP/C0001.MP4"));
        assert!(!is_named_after("D:/CLIP/C0002.MP4", "D:/CLIP/C0001.MP4"));
        assert!(!is_named_after("D:/CLIP/C0001.MP4", ""));
    }

    #[test]
    fn a_clip_new_to_the_effect_sets_what_follows_it() {
        let mut s = StoredParams::default();
        // Set up on a square-pixel clip of a 4K sequence
        s.on_clip((3840, 2160), false, true);
        assert_eq!(s.sequence_size, (3840, 2160));
        assert!(!s.pending_params_bool.contains_key(&Params::DisableStretch));
        // Copied to an anamorphic clip of a 1080p sequence
        s.on_clip((1920, 1080), true, false);
        assert_eq!(s.sequence_size, (1920, 1080));
        assert_eq!(s.pending_params_bool.get(&Params::DisableStretch), Some(&true));
        // Back to a square-pixel one, with an output size of the user's
        s.sequence_size = (0, 0);
        s.on_clip((3840, 2160), false, false);
        assert_eq!(s.sequence_size, (0, 0));
        assert_eq!(s.pending_params_bool.get(&Params::DisableStretch), Some(&false));
    }

    #[test]
    fn moves_to_another_clip_with_the_adjustments() {
        let (dir, _) = folder(&["C0001.MP4", "C0001.gyroflow", "C0002.MP4", "C0002.gyroflow"]);
        let (a, b) = (dir.path("C0001.MP4"), dir.path("C0002.MP4"));

        let mut s = stored(&a, &dir.path("C0001.gyroflow"));
        s.pending_params_str.insert(Params::ProjectData, "{}".into());
        s.pending_params_str.insert(Params::LoadedProject, "C0001.gyroflow".into());
        s.pending_params_str.insert(Params::EmbeddedPreset, "preset".into());
        s.pending_params_str.insert(Params::InstanceId, s.instance_id.clone());
        s.pending_params_f64.insert(Params::Smoothness, 20.0);
        s.pending_params_f64.insert(Params::InputRotation, 90.0);
        s.pending_params_bool.insert(Params::DisableStretch, true);
        s.sequence_size = (3840, 2160);
        s.media_fps_ticks = 1001;
        s.speed_per_frame = vec![100.0];
        let instance_id = s.instance_id.clone();

        assert_eq!(follow(&mut s, &b), MediaChange::OtherClip);
        assert_eq!(s.media_file_path, b);
        assert_eq!(project_path(&s), dir.path("C0002.gyroflow"));
        assert_ne!(s.instance_id, instance_id);
        // Set to the parameter with the next edit
        assert_eq!(s.pending_params_str.get(&Params::InstanceId), Some(&s.instance_id));
        // What belongs to the clip starts over, what came with the effect stays
        assert_eq!(s.pending_params_str.get(&Params::ProjectData).map(|x| x.as_str()), Some(""));
        assert!(!s.pending_params_str.contains_key(&Params::LoadedProject));
        assert!(!s.pending_params_f64.contains_key(&Params::InputRotation));
        assert!(!s.pending_params_bool.contains_key(&Params::DisableStretch));
        assert!(s.speed_per_frame.is_empty() && s.media_fps_ticks == 0);
        // Until the host sets it for the clip, it says the output size follows the sequence's (see `on_clip`)
        assert_eq!(s.sequence_size, (3840, 2160));
        assert_eq!(s.pending_params_f64.get(&Params::Smoothness), Some(&20.0));
        assert_eq!(s.pending_params_str.get(&Params::EmbeddedPreset).map(|x| x.as_str()), Some("preset"));
        // Moved once
        assert_eq!(follow(&mut s, &b), MediaChange::None);
    }

    #[test]
    fn another_clip_reloads_the_project_but_not_the_adjustments() {
        let (dir, _) = folder(&["C0001.MP4", "C0002.MP4"]);
        let (a, b) = (dir.path("C0001.MP4"), dir.path("C0002.MP4"));

        let mut inst = Instance::default();
        *inst.stored.write() = stored(&a, &a);
        inst.gyroflow.as_mut().unwrap().reload_values_from_project = false;
        let keyframes = inst.gyroflow.as_ref().unwrap().keyframable_params.clone();

        assert_eq!(inst.follow_media(&b, || (a.clone(), String::new())), MediaChange::OtherClip);
        let base = inst.gyroflow.as_ref().unwrap();
        assert!(base.reload_values_from_project && base.keep_adjustments && base.ever_changed);
        // The stabilization of the clip it was on keeps the keyframes it read
        assert!(!Arc::ptr_eq(&keyframes, &base.keyframable_params));
        // Not read again when the media is the one stored
        assert_eq!(inst.follow_media(&b, || unreachable!()), MediaChange::None);
    }

    #[test]
    fn string_params_save_as_strings() {
        /// The value as it was kept up to v2.1.1
        #[derive(Serialize)]
        struct OldArbString { val: String }
        let path = format!("D:/clips/{}.MP4", fastrand::u64(..));

        use parameters::ArbString;
        let path = format!("D:/clips/{}.MP4", fastrand::u64(..));

        // In the host's handle as it was saved before, and as a `String`, which Premiere's GPU filter reads it flattened as
        let value = ArbString::encode(&path);
        assert_eq!(value, bincode::serde::encode_to_vec(&OldArbString { val: path.clone() }, bincode::config::legacy()).unwrap());
        assert_eq!(value, bincode::serde::encode_to_vec(&path, bincode::config::legacy()).unwrap());
        assert_eq!(ArbString::decode(&value), Some(path.as_bytes()));
        // The default value
        assert_eq!(ArbString::decode(&ArbString::EMPTY.to_le_bytes()), Some(&b""[..]));
        assert_eq!(ArbString::encode(""), ArbString::EMPTY.to_le_bytes());
        // In a handle larger than it, still the value
        let larger = [&value[..], &[0; 4]].concat();
        assert_eq!(ArbString::decode(&larger), Some(path.as_bytes()));
        // Not a value
        assert_eq!(ArbString::decode(&value[..value.len() - 1]), None);
        assert_eq!(ArbString::decode(&[1, 2]), None);
        assert_eq!(ArbString::decode(&u64::MAX.to_le_bytes()), None);
    }
}
