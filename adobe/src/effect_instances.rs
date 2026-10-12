//! Which instance each effect uses.
//!
//! An instance lives in the map of `CrossThreadInstance`, under the id saved in the sequence data, so that the copies
//! the host makes of an effect's sequence data (for render threads, Premiere's render clones and its GPU filter) are all
//! the same instance. But the hosts also copy the sequence data with the effect: a pasted or duplicated effect, the
//! pieces of a cut clip, a preset applied again. Those are other effects, with instances of their own, so each effect
//! gets its own (see `EffectKey` for how far the hosts tell their effects apart):
//! - Sequence data seen for the first time (the project's, or a preset's) is the instance of the effect it comes with.
//! - Another effect's gets a copy of what was copied, made of the copied sequence data. The same one for every copy of
//!   these bytes the host restores for the effect. The copy shares the stabilization of the one it was copied from, and
//!   that one shares it from then on too (see `GyroflowPluginBaseInstance::ever_changed`).
//! - In After Effects, an instance set up on the UI thread is one effect's until it's flattened or disposed of there
//!   (`release`): another effect set up with it meanwhile is a copy, duplicated on the same layer or pasted onto it again.
//!   The effect set up with it again, its saved state is the one it has (a project reverted or opened again).
//! - In Premiere, where an effect has one instance, an instance it had before, which a render clone set up before is
//!   still given, stands for the one it has now, unless the effect is set up with it again (an undo)

use super::*;
use std::cell::RefCell;

/// An effect in the host's project, as far as the plugin can tell them apart
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectKey {
    /// Premiere's filter instance ID (`PF_UtilitySuite::GetFilterInstanceID`, `Effect_RuntimeInstanceID` in the GPU
    /// filter): the effect's, the same for its render clones and its GPU filter, and of no other effect in the session
    Filter(u32),
    /// The layer of an effect in After Effects: its comp's item ID and its own ID. After Effects has no ID for the effect
    /// itself, and these are only the project's (another project opened has the same ones, so does a layer of a deleted
    /// one maybe), so a layer can have several effects' instances. Read on render threads too, unlike the effect's
    /// streams, which After Effects doesn't let them dispose of
    Layer(i32, u32),
}
impl EffectKey {
    /// Whether it's of one effect only, which has one instance
    fn one_effect(&self) -> bool {
        matches!(self, Self::Filter(_))
    }
    /// The effect `in_data` is for, `None` when the host can't tell (a layer being deleted, or not set up yet)
    pub fn of(in_data: &InData) -> Option<Self> {
        if in_data.is_premiere() {
            return in_data.effect().filter_instance_id().ok().map(|x| Self::Filter(x as u32));
        }
        let layer = in_data.effect().layer().ok()?;
        Some(Self::Layer(layer.parent_comp().ok()?.item().ok()?.id().ok()?, layer.id().ok()?))
    }
}

#[derive(Default)]
struct EffectInstances {
    /// The instance of each of Premiere's effects
    current: HashMap<EffectKey, u64>,
    /// The effect of each instance
    owner: HashMap<u64, EffectKey>,
    /// The copy made for an effect of another effect's instance, by the hash of the copied sequence data (0 for a copy
    /// made without it)
    copies: HashMap<(EffectKey, u64, u64), u64>,
    /// The last copy made for an effect of another effect's instance
    last_copy: HashMap<(EffectKey, u64), u64>,
    /// The instances After Effects' UI thread has set up for an effect, and not flattened or disposed of since
    live: HashMap<u64, usize>,
    /// The instances copies were made of, which share their stabilization with them from now on (see `take_copied`)
    copied: HashSet<u64>,
}
static EFFECTS: std::sync::LazyLock<parking_lot::Mutex<EffectInstances>> = std::sync::LazyLock::new(Default::default);

/// The instance `id` was flattened or disposed of on After Effects' UI thread: no effect there has it anymore. One that
/// isn't an instance does nothing (the flat sequence data of an effect pasted onto itself, which has no id)
pub fn release(id: u64) {
    let mut effects = EFFECTS.lock();
    if let Some(n) = effects.live.get_mut(&id) {
        *n -= 1;
        if *n == 0 {
            effects.live.remove(&id);
        }
    }
}
/// Whether a copy was made of the instance `id` since the last call, which shares the stabilization with it from now on
pub fn take_copied(id: u64) -> bool {
    EFFECTS.lock().copied.remove(&id)
}

thread_local! {
    /// The sequence data `AdobePluginInstance::unflatten` restored on this thread, for the `SequenceResetup` sent with it
    static RESTORED: RefCell<Option<(u16, Vec<u8>)>> = const { RefCell::new(None) };
}
/// Keeps the sequence data restored for the `SequenceResetup` being handled on this thread (see `take_restored`)
pub fn set_restored(version: u16, bytes: &[u8]) {
    RESTORED.with(|x| *x.borrow_mut() = Some((version, bytes.to_vec())));
}
/// The sequence data restored for the `SequenceResetup` of the instance `id` handled on this thread
pub fn take_restored(id: u64) -> Option<(u16, Vec<u8>)> {
    RESTORED.with(|x| x.borrow_mut().take()).filter(|(_, bytes)| sequence_data::saved_id(bytes) == Some(id))
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{ Hash, Hasher };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl EffectInstances {
    fn alive(id: &u64) -> bool {
        CrossThreadInstance::map().read().contains_key(id)
    }
    fn current(&self, key: EffectKey) -> Option<u64> {
        self.current.get(&key).copied().filter(Self::alive)
    }
    /// Makes `id` the instance of `key`, its current one in Premiere when `current`
    fn bind(&mut self, key: EffectKey, id: u64, current: bool) -> u64 {
        self.owner.insert(id, key);
        if current && key.one_effect() {
            self.current.insert(key, id);
        }
        id
    }
    /// The effect set up with the instance `id` on After Effects' UI thread (`ui`) has it, until it's released
    fn take(&mut self, id: u64, ui: bool) {
        if ui {
            *self.live.entry(id).or_default() += 1;
        }
    }
    /// The copy made for `key` of the instance `source`, of the sequence data with `hash` (any when `None`)
    fn copy_of(&self, key: EffectKey, source: u64, hash: Option<u64>) -> Option<u64> {
        match hash {
            Some(hash) => self.copies.get(&(key, source, hash)),
            None => self.last_copy.get(&(key, source)),
        }.copied().filter(Self::alive)
    }
}

impl CrossThreadInstance {
    /// The instance of the effect `key`, given sequence data with this one.
    ///
    /// `setup` is for the sequence data the host restores the effect with (`SequenceResetup`, but for a render thread's):
    /// another effect's is a copy of it then, pasted or applied as a preset, which the effect now has. Other sequence data
    /// may be older than the effect's, a render clone's set up before. `saved` is the sequence data as the host keeps it,
    /// to make a copy of. `ui` is for the effect set up on After Effects' UI thread, which has the instance until it's
    /// released (see `release`)
    pub fn of_effect(&self, key: EffectKey, setup: bool, saved: Option<(u16, &[u8])>, ui: bool) -> Self {
        // Another effect of the UI thread has it: the same layer can have several (see `EffectKey::Layer`)
        let ui = ui && !key.one_effect();
        let held = |effects: &EffectInstances, id: u64| ui && effects.live.contains_key(&id);
        let (source_effect, hash) = {
            let mut effects = EFFECTS.lock();
            let current = effects.current(key);
            match effects.owner.get(&self.id).copied() {
                // The effect's own. In Premiere, an older one stands for the current one, but set up with it again. Or seen
                // for the first time: the project's or a preset's, the effect's when it's set up with it. In Premiere, other
                // sequence data than the effect's instance otherwise
                owner if owner.is_none() || (owner == Some(key) && !held(&effects, self.id)) => {
                    let id = match current {
                        Some(id) if !setup => id,
                        _ => effects.bind(key, self.id, true)
                    };
                    effects.take(id, ui);
                    drop(effects);
                    // In After Effects, set up with it again, the effect has its saved state (a project reverted or opened
                    // again), not the one it had when it was disposed of
                    if let Some((version, bytes)) = saved.filter(|_| ui && setup && owner.is_some()) {
                        Self { id }.restore_saved(version, bytes);
                    }
                    return Self { id };
                }
                owner => {
                    let source_effect = owner.unwrap_or(key);
                    let hash = saved.map(|x| hash_bytes(x.1)).unwrap_or_default();
                    // Another effect's, a copy of which the effect has already, unless another effect has that one. Not set
                    // up with it, it's older sequence data than the effect's in Premiere
                    let made = if setup {
                        effects.copy_of(key, self.id, Some(hash))
                    } else {
                        current.or_else(|| effects.copy_of(key, self.id, Some(hash))).or_else(|| effects.copy_of(key, self.id, None))
                    }.filter(|id| !held(&effects, *id));
                    if let Some(id) = made {
                        let id = effects.bind(key, id, setup || current.is_none());
                        effects.take(id, ui);
                        return Self { id };
                    }
                    (source_effect, hash)
                }
            }
        };

        // Another effect's: copied with this one. Made without the lock, as the instance copied may be held for long (a
        // render loading its project)
        let copy = saved.and_then(|(version, bytes)| sequence_data::decode(version, bytes).map_err(|e| log::error!("Failed to copy the saved instance: {e}")).ok())
                        .map(|(_, inst)| Self::insert(inst))
                        .unwrap_or_else(|| self.copy());
        if let Some(inst) = copy.get() {
            let mut inst = inst.write();
            inst.ensure_base();
            // Shares the stabilization of the one it was copied from until changed, as the pieces of a cut clip do
            if let Some(base) = inst.gyroflow.as_mut() { base.ever_changed = false; }
            // Session state of the effect it was copied from
            let mut stored = inst.stored.write();
            stored.keyframes_state.clear();
            stored.keyframable_set_to.clear();
        }

        let mut effects = EFFECTS.lock();
        if let Some(id) = effects.copy_of(key, self.id, Some(hash)).filter(|id| !held(&effects, *id)) {
            // Made by another thread meanwhile
            Self::map().write().remove(&copy.id);
            effects.take(id, ui);
            return Self { id };
        }
        log::info!("Effect {key:?} has a copy of the instance {} of the effect {source_effect:?}: {}", self.id, copy.id);
        effects.copies.insert((key, self.id, hash), copy.id);
        effects.last_copy.insert((key, self.id), copy.id);
        // And the one it was copied from shares the stabilization with it now. Set right away, unless a render holds it, and
        // otherwise when it next has its lock, or is saved
        effects.copied.insert(self.id);
        let id = effects.bind(key, copy.id, true);
        effects.take(id, ui);
        drop(effects);
        if let Some(mut source) = self.get().as_ref().and_then(|x| x.try_write()) {
            source.sync_copies(self.id);
        }
        Self { id }
    }

    /// Gives the instance the state saved in `bytes` by a build of `version`, when it has changed since
    fn restore_saved(&self, version: u16, bytes: &[u8]) {
        let Some(inst) = self.get() else { return; };
        if self.flatten().is_ok_and(|(v, current)| v == version && current == bytes) {
            return;
        }
        match sequence_data::decode(version, bytes) {
            Ok((_, saved)) => {
                log::info!("Instance {} restored to its saved state", self.id);
                let mut inst = inst.write();
                // In place: what the host still renders with has the same
                *inst.stored.write() = saved.stored.read().clone();
                inst.gyroflow = saved.gyroflow;
                inst.ensure_base();
            }
            Err(e) => log::error!("Failed to restore the saved instance {}: {e}", self.id),
        }
    }

    /// The instance of the effect `key` for sequence data with this one, as `of_effect` made it: for a render thread of
    /// After Effects, which works on a copy of the project, where nothing says the layers are the UI's. `None` when it
    /// hasn't been made yet
    pub fn found_for(&self, key: EffectKey) -> Option<Self> {
        let effects = EFFECTS.lock();
        let id = if key.one_effect() {
            effects.current(key)
        } else {
            match effects.owner.get(&self.id) {
                Some(owner) if *owner == key => Some(self.id),
                Some(_) => effects.copy_of(key, self.id, None),
                None => None,
            }
        };
        id.map(|id| Self { id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys of their own, the map is shared by the tests running at the same time
    fn filters() -> (EffectKey, EffectKey) {
        (EffectKey::Filter(fastrand::u32(..u32::MAX / 2)), EffectKey::Filter(fastrand::u32(u32::MAX / 2..)))
    }
    fn layers() -> (EffectKey, EffectKey) {
        (EffectKey::Layer(fastrand::i32(..), fastrand::u32(..u32::MAX / 2)), EffectKey::Layer(fastrand::i32(..), fastrand::u32(u32::MAX / 2..)))
    }
    fn media(inst: &CrossThreadInstance) -> String {
        inst.get().unwrap().read().stored.read().media_file_path.clone()
    }
    fn set_media(inst: &CrossThreadInstance, path: &str) {
        inst.get().unwrap().read().stored.write().media_file_path = path.into();
    }

    #[test]
    fn copied_effects_get_their_own_instance() {
        for (a, b) in [filters(), layers()] {
            let original = CrossThreadInstance::default();
            set_media(&original, "A.MP4");
            assert_eq!(original.of_effect(a, true, None, false).id, original.id);
            assert_eq!(original.of_effect(a, false, None, false).id, original.id);

            // The sequence data pasted to another effect, as it was when copied
            let (version, saved) = original.flatten().unwrap();
            set_media(&original, "A2.MP4");
            let pasted = CrossThreadInstance { id: original.id };
            let copy = pasted.of_effect(b, true, Some((version, &saved)), false);
            assert_ne!(copy.id, original.id);
            assert_eq!(media(&copy), "A.MP4");
            assert!(!copy.get().unwrap().read().gyroflow.as_ref().unwrap().ever_changed);
            assert!(!Arc::ptr_eq(&copy.get().unwrap().read().stored, &original.get().unwrap().read().stored));

            // The same sequence data restored again for that effect, by its render clones
            assert_eq!(pasted.of_effect(b, true, Some((version, &saved)), false).id, copy.id);
            assert_eq!(pasted.of_effect(b, false, None, false).id, copy.id);
            assert_eq!(pasted.found_for(b).map(|x| x.id), Some(copy.id));
            // The original stays the original's
            assert_eq!(original.of_effect(a, false, None, false).id, original.id);
            assert_eq!(original.found_for(a).map(|x| x.id), Some(original.id));
        }
    }

    #[test]
    fn copies_are_made_without_the_sequence_data() {
        for (a, b) in [filters(), layers()] {
            let original = CrossThreadInstance::default();
            set_media(&original, "A.MP4");
            original.of_effect(a, true, None, false);

            // A render clone of the copy, before the effect's own sequence data was set up
            let copy = CrossThreadInstance { id: original.id }.of_effect(b, false, None, false);
            assert_ne!(copy.id, original.id);
            assert_eq!(media(&copy), "A.MP4");
            assert_eq!(CrossThreadInstance { id: original.id }.of_effect(b, false, None, false).id, copy.id);
        }
    }

    #[test]
    fn premiere_render_clones_from_before_get_the_current_instance() {
        let (a, b) = filters();
        let mine = CrossThreadInstance::default();
        mine.of_effect(b, true, None, false);
        let other = CrossThreadInstance::default();
        other.of_effect(a, true, None, false);

        // Another effect's sequence data pasted over this one's
        let (version, saved) = other.flatten().unwrap();
        let copy = CrossThreadInstance { id: other.id }.of_effect(b, true, Some((version, &saved)), false);
        assert_ne!(copy.id, other.id);

        // A render clone set up before has the instance from before, the GPU filter maybe too
        assert_eq!(mine.of_effect(b, false, None, false).id, copy.id);
        assert_eq!(mine.found_for(b).map(|x| x.id), Some(copy.id));
        // A render clone set up with the pasted sequence data, again
        assert_eq!(CrossThreadInstance { id: other.id }.of_effect(b, false, Some((version, &saved)), false).id, copy.id);

        // The paste undone: the effect set up with its sequence data from before
        assert_eq!(mine.of_effect(b, true, None, false).id, mine.id);
        assert_eq!(CrossThreadInstance { id: other.id }.of_effect(b, false, Some((version, &saved)), false).id, mine.id);
    }

    #[test]
    fn pasted_again_after_a_change_is_another_copy() {
        for (a, b) in [filters(), layers()] {
            let original = CrossThreadInstance::default();
            original.of_effect(a, true, None, false);
            let (version, saved) = original.flatten().unwrap();
            let first = CrossThreadInstance { id: original.id }.of_effect(b, true, Some((version, &saved)), false);

            set_media(&original, "B.MP4");
            let (version, saved) = original.flatten().unwrap();
            let second = CrossThreadInstance { id: original.id }.of_effect(b, true, Some((version, &saved)), false);
            assert_ne!(second.id, first.id);
            assert_eq!(media(&second), "B.MP4");
        }
    }

    #[test]
    fn sequence_data_seen_first_in_premiere() {
        let (filter, _) = filters();
        let loaded = CrossThreadInstance::default();
        assert_eq!(loaded.of_effect(filter, false, None, false).id, loaded.id);

        // Another effect's attributes, from another session, pasted over it
        let pasted = CrossThreadInstance::default();
        assert_eq!(pasted.of_effect(filter, true, None, false).id, pasted.id);
        assert_eq!(loaded.of_effect(filter, false, None, false).id, pasted.id);
        // Sequence data the effect hasn't been set up with
        assert_eq!(CrossThreadInstance::default().of_effect(filter, false, None, false).id, pasted.id);
    }

    #[test]
    fn after_effects_layers_keep_the_instances_of_their_effects() {
        let (layer, other_layer) = layers();
        let first = CrossThreadInstance::default();
        assert_eq!(first.of_effect(layer, false, None, false).id, first.id);

        // Another effect applied to the layer, the effect deleted and applied again, or another project opened with a
        // layer of the same IDs: all their own
        for setup in [false, true] {
            let another = CrossThreadInstance::default();
            assert_eq!(another.of_effect(layer, setup, None, false).id, another.id);
            assert_eq!(another.found_for(layer).map(|x| x.id), Some(another.id));
        }
        assert_eq!(first.of_effect(layer, false, None, false).id, first.id);

        // A copy pasted onto the layer
        let elsewhere = CrossThreadInstance::default();
        elsewhere.of_effect(other_layer, true, None, false);
        let (version, saved) = elsewhere.flatten().unwrap();
        let pasted = CrossThreadInstance { id: elsewhere.id }.of_effect(layer, true, Some((version, &saved)), false);
        assert_ne!(pasted.id, elsewhere.id);
        assert_eq!(first.of_effect(layer, false, None, false).id, first.id);

        // A render thread of the layer: the instances made for it only
        assert_eq!(first.found_for(layer).map(|x| x.id), Some(first.id));
        assert_eq!(elsewhere.found_for(layer).map(|x| x.id), Some(pasted.id));
        assert!(CrossThreadInstance::default().found_for(layer).is_none());
    }

    /// The effect set up with the sequence data `saved` of the instance `id` on After Effects' UI thread, newly when it has none
    fn set_up(id: u64, key: EffectKey, saved: Option<&(u16, Vec<u8>)>) -> CrossThreadInstance {
        CrossThreadInstance { id }.of_effect(key, saved.is_some(), saved.map(|(v, b)| (*v, &b[..])), true)
    }

    #[test]
    fn effects_duplicated_on_their_layer_get_their_own_instance() {
        let (layer, _) = layers();
        let original = CrossThreadInstance::default();
        set_media(&original, "A.MP4");
        assert_eq!(set_up(original.id, layer, None).id, original.id);

        // Duplicated twice: the original keeps its instance, each duplicate gets one of its own
        let saved = original.flatten().unwrap();
        let first = set_up(original.id, layer, Some(&saved));
        let second = set_up(original.id, layer, Some(&saved));
        assert!(first.id != original.id && second.id != original.id && first.id != second.id);
        assert_eq!(media(&first), "A.MP4");
        assert_eq!(original.found_for(layer).map(|x| x.id), Some(original.id));
        assert_eq!(first.found_for(layer).map(|x| x.id), Some(first.id));

        // Deleted, and the deletion undone: set up with its own sequence data, which it has
        let own = first.flatten().unwrap();
        release(first.id);
        assert_eq!(set_up(first.id, layer, Some(&own)).id, first.id);
    }

    #[test]
    fn effects_pasted_twice_onto_a_layer_get_an_instance_each() {
        let (layer, other_layer) = layers();
        let source = CrossThreadInstance::default();
        set_up(source.id, other_layer, None);
        let saved = source.flatten().unwrap();
        let first = set_up(source.id, layer, Some(&saved));
        let second = set_up(source.id, layer, Some(&saved));
        assert!(first.id != source.id && second.id != source.id && first.id != second.id);

        // Deleted and the deletion undone: set up with its own sequence data
        let own = second.flatten().unwrap();
        release(second.id);
        assert_eq!(set_up(second.id, layer, Some(&own)).id, second.id);
        // Its paste undone and done again: the copy it had, which no other effect has
        release(second.id);
        assert_eq!(set_up(source.id, layer, Some(&saved)).id, second.id);
    }

    #[test]
    fn effects_set_up_again_have_their_saved_state() {
        let (layer, _) = layers();
        let inst = CrossThreadInstance::default();
        set_media(&inst, "A.MP4");
        set_up(inst.id, layer, None);
        let saved = inst.flatten().unwrap();
        let (instance, stored) = (inst.get().unwrap(), inst.get().unwrap().read().stored.clone());

        // Changed and not saved, and the project reverted: closed, and opened again
        set_media(&inst, "B.MP4");
        release(inst.id);
        assert_eq!(set_up(inst.id, layer, Some(&saved)).id, inst.id);
        assert_eq!(media(&inst), "A.MP4");
        // In place, for what the host still renders with
        assert!(Arc::ptr_eq(&instance, &inst.get().unwrap()) && Arc::ptr_eq(&stored, &inst.get().unwrap().read().stored));

        // Saved before: as it is
        set_media(&inst, "C.MP4");
        let saved = inst.flatten().unwrap();
        release(inst.id);
        assert_eq!(set_up(inst.id, layer, Some(&saved)).id, inst.id);
        assert_eq!(media(&inst), "C.MP4");
    }

    #[test]
    fn effects_copied_share_their_stabilization_with_the_copies() {
        for (a, b) in [filters(), layers()] {
            let original = CrossThreadInstance::default();
            original.get().unwrap().write().gyroflow.as_mut().unwrap().ever_changed = true;
            original.of_effect(a, true, None, false);
            let (version, saved) = original.flatten().unwrap();
            let copy = CrossThreadInstance { id: original.id }.of_effect(b, true, Some((version, &saved)), false);
            assert!(!copy.get().unwrap().read().gyroflow.as_ref().unwrap().ever_changed);
            assert!(!original.get().unwrap().read().gyroflow.as_ref().unwrap().ever_changed);
            assert!(!take_copied(original.id));
        }
    }

    #[test]
    fn effects_copied_while_rendering_share_their_stabilization_once_saved() {
        let (a, b) = layers();
        let original = CrossThreadInstance::default();
        original.get().unwrap().write().gyroflow.as_mut().unwrap().ever_changed = true;
        original.of_effect(a, true, None, false);
        let (version, saved) = original.flatten().unwrap();

        // A render holds it meanwhile, and it's saved before it renders again
        let inst = original.get().unwrap();
        let render = inst.read();
        CrossThreadInstance { id: original.id }.of_effect(b, true, Some((version, &saved)), false);
        assert!(render.gyroflow.as_ref().unwrap().ever_changed);
        drop(render);
        let (version, saved) = original.flatten().unwrap();
        let (_, saved) = sequence_data::decode(version, &saved).unwrap();
        assert!(!saved.gyroflow.unwrap().ever_changed);
    }
}
