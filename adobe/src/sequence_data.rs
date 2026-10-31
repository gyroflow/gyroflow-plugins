//! The instance as it's saved with the project (the effect's sequence data): the cross-thread id, the stored parameters,
//! then what's saved of the base instance, in bincode. bincode has no field names or lengths, just the fields one after
//! another, so a change in the layout makes the projects saved before undecodable. Decoding stops after the fields it
//! knows, so fields are only ever added at the end of the base instance's, the last thing saved, and `VERSION` says which
//! are there:
//! 1. Up to v2.1.1: `SavedBase`, `always_set_input_rotation` and `opencl_disabled`. Builds of 5474899 saved
//!    `fixed_output_size`, `last_out_size` and `opencl_disabled` after `SavedBase` instead, so only `SavedBase` is read
//! 2. `keep_adjustments` added

use super::*;
use serde::{ Serializer, Deserializer };

pub const VERSION: u16 = 2;

/// What's saved of `GyroflowPluginBaseInstance`, in the order of its fields up to v2.1.1
#[derive(Serialize, Deserialize)]
struct SavedBase {
    reload_values_from_project: bool,
    original_video_size: (usize, usize),
    original_output_size: (usize, usize),
    timeline_size: (usize, usize),
    num_frames: usize,
    fps: f64,
    has_motion: bool,
    ever_changed: bool,
    cache_keyframes_every_frame: bool,
    framebuffer_inverted: bool,
    anamorphic_adjust_size: bool,
}
/// Followed by the fields of version 1 (`always_set_input_rotation` and `opencl_disabled`, unused now, older builds read
/// them) and of version 2 (`keep_adjustments`)
type SavedBaseV2 = (SavedBase, bool, bool, bool);

impl SavedBase {
    fn new(x: &GyroflowPluginBaseInstance) -> Self {
        Self {
            reload_values_from_project:  x.reload_values_from_project,
            original_video_size:         x.original_video_size,
            original_output_size:        x.original_output_size,
            timeline_size:               x.timeline_size,
            num_frames:                  x.num_frames,
            fps:                         x.fps,
            has_motion:                  x.has_motion,
            ever_changed:                x.ever_changed,
            cache_keyframes_every_frame: x.cache_keyframes_every_frame,
            framebuffer_inverted:        x.framebuffer_inverted,
            anamorphic_adjust_size:      x.anamorphic_adjust_size,
        }
    }
    /// The settings for the Adobe hosts are this build's, not the saved ones
    fn restore(self, keep_adjustments: bool) -> GyroflowPluginBaseInstance {
        let mut base = CrossThreadInstance::base_instance();
        base.reload_values_from_project = self.reload_values_from_project;
        base.keep_adjustments           = keep_adjustments;
        base.original_video_size        = self.original_video_size;
        base.original_output_size       = self.original_output_size;
        base.timeline_size              = self.timeline_size;
        base.num_frames                 = self.num_frames;
        base.fps                        = self.fps;
        base.has_motion                 = self.has_motion;
        base.ever_changed               = self.ever_changed;
        base
    }
}

pub fn ser_base<S: Serializer>(x: &Option<GyroflowPluginBaseInstance>, s: S) -> Result<S::Ok, S::Error> {
    // `always_set_input_rotation` was on in the Adobe hosts
    x.as_ref().map(|x| (SavedBase::new(x), true, x.opencl_disabled, x.keep_adjustments)).serialize(s)
}
pub fn de_base<'de, D: Deserializer<'de>>(d: D) -> Result<Option<GyroflowPluginBaseInstance>, D::Error> {
    Ok(Option::<SavedBaseV2>::deserialize(d)?.map(|(base, _, _, keep_adjustments)| base.restore(keep_adjustments)))
}

/// The instance saved by a build of `version` (see the module's description). It's the one in the map when it's there
/// already: the host restores the same instance many times (render threads, Premiere's GPU filter every frame), with
/// sequence data that can be older than the instance. Restored for the effect itself, it gets the saved state (see
/// `effect_instances`)
pub fn unflatten(version: u16, bytes: &[u8]) -> CrossThreadInstance {
    let id = saved_id(bytes);
    if let Some(id) = id.filter(|x| CrossThreadInstance::map().read().contains_key(x)) {
        return CrossThreadInstance { id };
    }
    match decode(version, bytes) {
        Ok((id, inst)) => {
            CrossThreadInstance::map().write().entry(id).or_insert_with(|| Arc::new(RwLock::new(inst)));
            CrossThreadInstance { id }
        }
        Err(e) => {
            // Under the saved id, for the same instance each time these bytes come back
            log::error!("Failed to restore the saved instance (version {version}, {} bytes): {e}", bytes.len());
            let id = id.unwrap_or_else(|| fastrand::u64(..));
            CrossThreadInstance::map().write().entry(id).or_insert_with(|| Arc::new(RwLock::new(Instance::default())));
            CrossThreadInstance { id }
        }
    }
}

/// The id of the instance saved in `bytes`
pub fn saved_id(bytes: &[u8]) -> Option<u64> {
    bytes.get(..8).map(|x| u64::from_le_bytes(x.try_into().unwrap()))
}

/// The instance saved by a build of `version`, and the id it was saved under, decoded whatever is in the map: an effect
/// copied from another gets what was copied, not what the other effect has now
pub fn decode(version: u16, bytes: &[u8]) -> Result<(u64, Instance), String> {
    if version >= 2 {
        // As `CrossThreadInstance` saves it: the id, then the instance
        Ok(bincode::serde::decode_from_slice::<(u64, Instance), _>(bytes, bincode::config::legacy()).map_err(|e| e.to_string())?.0)
    } else {
        let ((id, stored, base), _) = bincode::serde::decode_from_slice::<(u64, StoredParams, Option<SavedBase>), _>(bytes, bincode::config::legacy()).map_err(|e| e.to_string())?;
        Ok((id, Instance {
            stored: Arc::new(RwLock::new(stored)),
            gyroflow: base.map(|x| x.restore(false)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GyroflowPluginBaseInstance` as saved up to v2.1.1
    #[derive(Serialize, Deserialize, Default)]
    struct BaseV211 {
        reload_values_from_project: bool,
        original_video_size: (usize, usize),
        original_output_size: (usize, usize),
        timeline_size: (usize, usize),
        num_frames: usize,
        fps: f64,
        has_motion: bool,
        ever_changed: bool,
        cache_keyframes_every_frame: bool,
        framebuffer_inverted: bool,
        anamorphic_adjust_size: bool,
        always_set_input_rotation: bool,
        opencl_disabled: bool,
    }
    /// As saved by builds of 5474899
    #[derive(Serialize, Default)]
    struct Base5474899 {
        reload_values_from_project: bool,
        original_video_size: (usize, usize),
        original_output_size: (usize, usize),
        timeline_size: (usize, usize),
        num_frames: usize,
        fps: f64,
        has_motion: bool,
        ever_changed: bool,
        cache_keyframes_every_frame: bool,
        framebuffer_inverted: bool,
        anamorphic_adjust_size: bool,
        fixed_output_size: bool,
        last_out_size: (usize, usize),
        opencl_disabled: bool,
    }
    /// The cross-thread instance as saved, with the base instance in the layout `B`
    #[derive(Serialize, Deserialize)]
    struct Saved<B> {
        id: u64,
        stored: StoredParams,
        gyroflow: Option<B>,
    }

    fn encode<T: Serialize>(x: &T) -> Vec<u8> {
        bincode::serde::encode_to_vec(x, bincode::config::legacy()).unwrap()
    }
    fn stored() -> StoredParams {
        let mut stored = StoredParams::default();
        stored.media_file_path = "/Volumes/Media A/CLIP/C0001.MP4".into();
        stored.instance_id = "1234".into();
        stored.pending_params_str.insert(Params::ProjectPath, stored.media_file_path.clone());
        stored.pending_params_f64.insert(Params::Smoothness, 75.0);
        stored
    }
    fn check_restored(inst: &CrossThreadInstance, keep_adjustments: bool) {
        let inst = inst.get().unwrap();
        let inst = inst.read();
        let stored = inst.stored.read();
        assert_eq!(stored.media_file_path, "/Volumes/Media A/CLIP/C0001.MP4");
        assert_eq!(stored.instance_id, "1234");
        assert_eq!(stored.pending_params_f64.get(&Params::Smoothness), Some(&75.0));
        let base = inst.gyroflow.as_ref().unwrap();
        assert!(!base.reload_values_from_project);
        assert!(base.ever_changed);
        assert_eq!(base.keep_adjustments, keep_adjustments);
        assert_eq!(base.original_video_size, (3840, 2160));
        assert_eq!(base.num_frames, 1234);
        assert_eq!(base.fps, 23.976);
    }

    #[test]
    fn restores_v2_1_1() {
        let id = fastrand::u64(..);
        let bytes = encode(&Saved { id, stored: stored(), gyroflow: Some(BaseV211 {
            ever_changed: true, original_video_size: (3840, 2160), num_frames: 1234, fps: 23.976, always_set_input_rotation: true, ..Default::default()
        }) });
        let inst = unflatten(1, &bytes);
        assert_eq!(inst.id, id);
        check_restored(&inst, false);
    }

    #[test]
    fn restores_5474899() {
        let id = fastrand::u64(..);
        // The low byte of 1920 isn't a valid bool, it mustn't be read as `opencl_disabled`
        let bytes = encode(&Saved { id, stored: stored(), gyroflow: Some(Base5474899 {
            ever_changed: true, original_video_size: (3840, 2160), num_frames: 1234, fps: 23.976, last_out_size: (1920, 1080), ..Default::default()
        }) });
        let inst = unflatten(1, &bytes);
        assert_eq!(inst.id, id);
        check_restored(&inst, false);
    }

    #[test]
    fn saves_for_older_builds_and_restores() {
        let inst = CrossThreadInstance::default();
        {
            let inst = inst.get().unwrap();
            let mut inst = inst.write();
            *inst.stored.write() = stored();
            let base = inst.gyroflow.as_mut().unwrap();
            base.reload_values_from_project = false;
            base.ever_changed = true;
            base.keep_adjustments = true;
            base.original_video_size = (3840, 2160);
            base.num_frames = 1234;
            base.fps = 23.976;
        }
        let (version, bytes) = inst.flatten().unwrap();
        assert_eq!(version, VERSION);

        // Decoded by a build up to v2.1.1
        let (old, _) = bincode::serde::decode_from_slice::<Saved<BaseV211>, _>(&bytes, bincode::config::legacy()).unwrap();
        assert_eq!(old.id, inst.id);
        assert_eq!(old.stored.media_file_path, "/Volumes/Media A/CLIP/C0001.MP4");
        let old_base = old.gyroflow.unwrap();
        assert!(!old_base.reload_values_from_project && old_base.ever_changed && old_base.always_set_input_rotation);
        assert_eq!(old_base.fps, 23.976);

        // Restored in a new session, without the instance in the map
        CrossThreadInstance::map().write().remove(&inst.id);
        let restored = unflatten(version, &bytes);
        assert_eq!(restored.id, inst.id);
        check_restored(&restored, true);
    }

    #[test]
    fn same_instance_for_the_same_bytes() {
        let bytes = encode(&Saved { id: fastrand::u64(..), stored: stored(), gyroflow: Some(BaseV211::default()) });
        let a = unflatten(1, &bytes);
        let b = unflatten(1, &bytes);
        assert!(Arc::ptr_eq(&a.get().unwrap(), &b.get().unwrap()));

        // Undecodable: a default instance, still the same one each time
        let mut garbage = fastrand::u64(..).to_le_bytes().to_vec();
        garbage.extend([0xff; 5]);
        let a = unflatten(2, &garbage);
        let b = unflatten(2, &garbage);
        assert!(Arc::ptr_eq(&a.get().unwrap(), &b.get().unwrap()));
        assert!(a.get().unwrap().read().gyroflow.is_some());
    }
}
