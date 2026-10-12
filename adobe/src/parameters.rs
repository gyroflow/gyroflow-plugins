
use gyroflow_plugin_base::*;
use after_effects as ae;
use premiere as pr;
use ae::{ ParamFlag, ParamUIFlags, ValueDisplayFlag };
use std::str::FromStr;
use serde::{ Serialize, Serializer, Deserialize, Deserializer };
use std::sync::Arc;
use parking_lot::RwLock;
use crate::StoredParams;

pub fn ticks_from_timetype(time: TimeType, ticks_per_frame: i64) -> i64 {
    match time {
        TimeType::Frame(x) => (x * ticks_per_frame as f64).round() as i64,
        TimeType::FrameOrMicrosecond((Some(x), _)) => (x * ticks_per_frame as f64).round() as i64,
        _ => panic!("Shouldn't happen"),
    }
}
pub fn ae_time_from_timetype(time: TimeType, time_step: i32, _time_scale: u32) -> i32 {
    match time {
        TimeType::Frame(x) => (x * time_step as f64).round() as i32,
        TimeType::FrameOrMicrosecond((Some(x), _)) => (x * time_step as f64).round() as i32,
        _ => panic!("Shouldn't happen"),
    }
}

pub fn ser_stored<S: Serializer>(x: &Arc<RwLock<StoredParams>>, s: S) -> Result<S::Ok, S::Error> {
    Serialize::serialize(&*x.read(), s)
}
pub fn de_stored<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<RwLock<StoredParams>>, D::Error> {
    let strs: StoredParams = Deserialize::deserialize(d)?;
    Ok(Arc::new(RwLock::new(strs)))
}

/// The value of the string parameters, as the host keeps it: the string itself, in the host's handle, laid out as it's
/// saved (bincode's `String`: the length as a little-endian u64, then the UTF-8 bytes; up to v2.1.1 it was a struct of
/// one, which saves the same), so projects read and write the same as before.
///
/// The host keeps the value in place in its handle, and copies the handle's bytes without asking (Premiere from other
/// threads, outside of the effect's lock), so the value can't own memory elsewhere, as a `String` would: the copies would
/// share it, and the first one changed or disposed would free it under the others. In the handle, each copy is a whole
/// value, and its memory goes when the host disposes of the handle
pub struct ArbString;

impl ArbString {
    const LEN: usize = std::mem::size_of::<u64>();
    /// The empty string, as the handle has it (its length), for `ArbitraryDef::set_default`
    pub const EMPTY: u64 = 0;

    /// `v` as the handle has it
    pub fn encode(v: &str) -> Vec<u8> {
        let mut bytes = vec![0; Self::LEN + v.len()];
        Self::put(&mut bytes, v.as_bytes());
        bytes
    }
    /// The string's bytes in a handle's `bytes`, `None` when they aren't a value
    pub fn decode(bytes: &[u8]) -> Option<&[u8]> {
        let len = u64::from_le_bytes(bytes.get(..Self::LEN)?.try_into().ok()?);
        bytes.get(Self::LEN..Self::LEN.checked_add(usize::try_from(len).ok()?)?)
    }
    /// Puts the value of the string's bytes `v` in `dst`, which is as long as it
    fn put(dst: &mut [u8], v: &[u8]) {
        dst[..Self::LEN].copy_from_slice(&(v.len() as u64).to_le_bytes());
        dst[Self::LEN..].copy_from_slice(v);
    }

    /// `f` of the string's bytes in the handle `h`, locked meanwhile. `None` for a null handle or one that isn't a value
    fn with_bytes<R>(h: ae::sys::PF_Handle, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        if h.is_null() { return None; }
        let suite = ae::pf::suites::Handle::new().ok()?;
        let ptr = suite.lock_handle(h) as *const u8;
        if ptr.is_null() { return None; }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, suite.handle_size(h) as usize) };
        let ret = Self::decode(bytes).map(f);
        suite.unlock_handle(h);
        ret
    }
    /// Puts the value of the string's bytes `v` in the handle `*h`: a new one when it's null, resized in place otherwise
    fn set(h: &mut ae::sys::PF_Handle, v: &[u8]) -> Result<(), ae::Error> {
        let suite = ae::pf::suites::Handle::new()?;
        let size = Self::LEN + v.len();
        if h.is_null() {
            *h = suite.new_handle(size as _);
            if h.is_null() { return Err(ae::Error::OutOfMemory); }
        } else {
            suite.resize_handle(size, h)?;
        }
        let ptr = suite.lock_handle(*h) as *mut u8;
        if ptr.is_null() { return Err(ae::Error::OutOfMemory); }
        Self::put(unsafe { std::slice::from_raw_parts_mut(ptr, size) }, v);
        suite.unlock_handle(*h);
        Ok(())
    }
    /// A new handle of the value of the string's bytes `v`
    fn new_handle(v: &[u8]) -> Result<ae::sys::PF_Handle, ae::Error> {
        let mut h = std::ptr::null_mut();
        Self::set(&mut h, v)?;
        Ok(h)
    }
    /// A new handle of the value in the handle `h`, the empty string when it has none
    fn copy(h: ae::sys::PF_Handle) -> Result<ae::sys::PF_Handle, ae::Error> {
        Self::with_bytes(h, Self::new_handle).unwrap_or_else(|| Self::new_handle(b""))
    }

    /// The string in the handle `h`
    pub fn read(h: ae::sys::PF_Handle) -> String {
        Self::with_bytes(h, |x| String::from_utf8_lossy(x).into_owned()).unwrap_or_default()
    }
    /// Sets the string in the handle `*h` to `v`. Returns whether it changed
    pub fn write(h: &mut ae::sys::PF_Handle, v: &str) -> Result<bool, ae::Error> {
        if Self::with_bytes(*h, |x| x == v.as_bytes()) == Some(true) {
            return Ok(false);
        }
        Self::set(h, v.as_bytes())?;
        Ok(true)
    }

    /// Handles the host's calls for the values of the string parameters, which all the arbitrary data parameters are
    pub fn callback(extra: &ae::ArbParamsExtra) -> Result<(), ae::Error> {
        use ae::sys;
        let x = extra.as_ref();
        unsafe {
            match x.which_function as _ {
                sys::PF_Arbitrary_NEW_FUNC => {
                    *x.u.new_func_params.arbPH = Self::new_handle(b"")?;
                }
                sys::PF_Arbitrary_DISPOSE_FUNC => {
                    let h = x.u.dispose_func_params.arbH;
                    if !h.is_null() {
                        ae::pf::suites::Handle::new()?.dispose_handle(h);
                    }
                }
                sys::PF_Arbitrary_COPY_FUNC => {
                    *x.u.copy_func_params.dst_arbPH = Self::copy(x.u.copy_func_params.src_arbH)?;
                }
                // Not time-varying: the value of the left one
                sys::PF_Arbitrary_INTERP_FUNC => {
                    *x.u.interp_func_params.interpPH = Self::copy(x.u.interp_func_params.left_arbH)?;
                }
                sys::PF_Arbitrary_FLAT_SIZE_FUNC => {
                    let len = Self::with_bytes(x.u.flat_size_func_params.arbH, |x| x.len()).unwrap_or_default();
                    *x.u.flat_size_func_params.flat_data_sizePLu = (Self::LEN + len) as _;
                }
                // Flat, it's as in the handle
                sys::PF_Arbitrary_FLATTEN_FUNC => {
                    let p = &x.u.flatten_func_params;
                    if p.flat_dataPV.is_null() { return Err(ae::Error::InvalidParms); }
                    let flat = std::slice::from_raw_parts_mut(p.flat_dataPV as *mut u8, p.buf_sizeLu as usize);
                    let mut put = |v: &[u8]| flat.get_mut(..Self::LEN + v.len()).map(|dst| Self::put(dst, v)).is_some();
                    let put = match Self::with_bytes(p.arbH, &mut put) {
                        Some(put) => put,
                        None => put(b""),
                    };
                    if !put {
                        return Err(ae::Error::InvalidParms);
                    }
                }
                sys::PF_Arbitrary_UNFLATTEN_FUNC => {
                    let p = &x.u.unflatten_func_params;
                    if p.flat_dataPV.is_null() { return Err(ae::Error::InvalidParms); }
                    let flat = std::slice::from_raw_parts(p.flat_dataPV as *const u8, p.buf_sizeLu as usize);
                    *p.arbPH = Self::new_handle(Self::decode(flat).ok_or(ae::Error::InternalStructDamaged)?)?;
                }
                sys::PF_Arbitrary_COMPARE_FUNC => {
                    let p = &x.u.compare_func_params;
                    let ordering = if p.a_arbH == p.b_arbH {
                        std::cmp::Ordering::Equal
                    } else {
                        let with_b = |a: &[u8]| Self::with_bytes(p.b_arbH, |b| a.cmp(b)).unwrap_or(a.cmp(b""));
                        Self::with_bytes(p.a_arbH, with_b).unwrap_or_else(|| with_b(b""))
                    };
                    *p.compareP = match ordering {
                        std::cmp::Ordering::Less    => sys::PF_ArbCompare_LESS,
                        std::cmp::Ordering::Equal   => sys::PF_ArbCompare_EQUAL,
                        std::cmp::Ordering::Greater => sys::PF_ArbCompare_MORE,
                    } as _;
                }
                // As text, a JSON string
                sys::PF_Arbitrary_PRINT_SIZE_FUNC => {
                    let p = &x.u.print_size_func_params;
                    *p.print_sizePLu = (serde_json::to_string(&Self::read(p.arbH)).unwrap_or_default().len() + 1) as _;
                }
                sys::PF_Arbitrary_PRINT_FUNC => {
                    let p = &x.u.print_func_params;
                    let text = std::ffi::CString::new(serde_json::to_string(&Self::read(p.arbH)).unwrap_or_default()).unwrap_or_default();
                    let text = text.as_bytes_with_nul();
                    if text.len() <= p.print_sizeLu as usize && p.print_flags == 0 && !p.print_bufferPC.is_null() {
                        std::ptr::copy_nonoverlapping(text.as_ptr(), p.print_bufferPC as *mut u8, text.len());
                    }
                }
                sys::PF_Arbitrary_SCAN_FUNC => {
                    let p = &x.u.scan_func_params;
                    let text = std::ffi::CStr::from_ptr(p.bufPC).to_str().map_err(|_| ae::Error::InternalStructDamaged)?;
                    let value = serde_json::from_str::<String>(text).map_err(|_| ae::Error::InternalStructDamaged)?;
                    *p.arbPH = Self::new_handle(value.as_bytes())?;
                }
                _ => return Err(ae::Error::Generic)
            }
        }
        Ok(())
    }
}

pub fn define_param(params: &mut ae::Parameters<Params>, x: ParameterType, _group: Option<&'static str>) {
    match x {
        ParameterType::HiddenString { id } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, id, ae::ArbitraryDef::setup(|f| {
                f.set_default(ArbString::EMPTY).unwrap();
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::NO_ECW_UI);
                -1
            }).unwrap();
        }
        ParameterType::TextBox { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, label, ae::ArbitraryDef::setup(|f| {
                f.set_default(ArbString::EMPTY).unwrap();
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::CONTROL);
                param.set_ui_width(250);
                param.set_ui_height(15);
                -1
            }).unwrap();
        }

        ParameterType::Button { id, label, .. } => {
            if id == "CreateCamera" && !params.in_data().is_after_effects() { return; }
            let p = Params::from_str(id).unwrap();
            if p == Params::LoadCurrent { return; }
            params.add_with_flags(p, "", ae::ButtonDef::setup(|f| { f.set_label(label); }), ParamFlag::SUPERVISE | ParamFlag::CANNOT_TIME_VARY, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Text { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, label, ae::ArbitraryDef::setup(|f| {
                f.set_default(ArbString::EMPTY).unwrap();
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::CONTROL | ae::ParamUIFlags::DO_NOT_ERASE_CONTROL);
                param.set_ui_width(250);
                param.set_ui_height(15*4);
                -1
            }).unwrap();
        }
        ParameterType::Slider { id, label, min, max, default, .. } => {
            let p = Params::from_str(id).unwrap();
            if p == Params::VideoSpeed { return; }
            if p == Params::FusionStartFrame { return; }
            params.add_with_flags(p, label, ae::FloatSliderDef::setup(|f| {
                f.set_valid_min(min as f32);
                f.set_slider_min(min as f32);
                f.set_valid_max(max as f32);
                f.set_slider_max(max as f32);
                f.set_value(default);
                f.set_default(default);
                f.set_precision(1);
                f.set_display_flags(ValueDisplayFlag::NONE);
            }), ParamFlag::SUPERVISE, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Checkbox { id, label, default, .. } => {
            if id == "DontDrawOutside" { return; }
            params.add_with_flags(Params::from_str(id).unwrap(), label, ae::CheckBoxDef::setup(|f| {
                f.set_default(default);
                f.set_value(default);
                f.set_label("");
            }), ParamFlag::SUPERVISE, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Select { id, label, options, default, .. } => {
            params.add_with_flags(Params::from_str(id).unwrap(), label, ae::PopupDef::setup(|f| {
                f.set_options(&options);
                f.set_default(options.iter().position(|x| *x == default).unwrap_or(0) as i32);
                f.set_value(f.default());
            }), ParamFlag::SUPERVISE, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Group { id, label, parameters, opened } => {
            if id == "InfoGroup" { return; }
            params.add_group(Params::from_str(id).unwrap(), Params::from_str(&format!("{id}End")).unwrap(), label, !opened, |params| {
                for x in parameters {
                    define_param(params, x, Some(id));
                }
                Ok(())
            }).unwrap();
        }
    }
}

pub fn param_index_for_type(type_: Params, init: Option<std::collections::HashMap<Params, ae::ParamMapInfo>>) -> Option<usize> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<Params, ae::ParamMapInfo>> = std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| init.unwrap());

    map.get(&type_).map(|x| x.index)
}


pub enum ParamsInner<'a, 'b> where 'b: 'a {
    Ae(&'a mut ae::Parameters<'b, Params>),
    AeRO(&'a ae::Parameters<'b, Params>),
    Premiere((&'a pr::GpuFilterData, pr::RenderParams))
}

pub struct ParamHandler<'a, 'b> where 'b: 'a {
    pub inner: ParamsInner<'a, 'b>,
    pub stored: Arc<RwLock<StoredParams>>,
}
/// The handle of the value of the string parameter `def` (see `ArbString`)
pub fn arb_handle(def: &ae::ParamDef) -> Result<ae::sys::PF_Handle, ae::Error> {
    def.as_arbitrary()?;
    Ok(unsafe { def.as_ref().u.arb_d.value })
}

fn is_keyframable(p: Params) -> bool {
    GyroflowPluginBaseInstance::KEYFRAMABLE_PARAMS.iter().any(|x| x.1 == p)
}

impl<'a, 'b> ParamHandler<'a, 'b> {
    /// The keyframable parameters whose values over time changed since the last call, told by the receipts of their state
    /// After Effects gives for caching. The first call takes the state only. So does one after the plugin set a parameter
    /// to the value it then has (see `set_f64`): the host applies the values set in an edit once it's handled, and they're
    /// cached already. `reported` is the parameter of the edit being handled, cached again with it. In After Effects only
    pub fn changed_keyframable_params(&self, reported: Option<Params>) -> Vec<Params> {
        let in_data = match &self.inner {
            ParamsInner::Ae(x) => x.in_data(),
            ParamsInner::AeRO(x) => x.in_data(),
            ParamsInner::Premiere(_) => return Vec::new(),
        };
        if !in_data.is_after_effects() { return Vec::new(); }
        let effect = in_data.effect();
        // A call to the host each, made without the lock: render threads read the stored parameters meanwhile. Of all time
        let states = GyroflowPluginBaseInstance::KEYFRAMABLE_PARAMS.iter().filter_map(|(_, p, _)| {
            Some((*p, effect.current_param_state(param_index_for_type(*p, None)? as i32, None, None).ok()?))
        }).collect::<Vec<_>>();
        let (last, set_to) = {
            let stored = self.stored.read();
            (stored.keyframes_state.clone(), stored.keyframable_set_to.clone())
        };

        let mut changed = Vec::new();
        let mut applied = reported.into_iter().collect::<Vec<_>>();
        for (p, state) in &states {
            let Some((_, last)) = last.iter().find(|(x, _)| x == p) else { continue; };
            // The same receipt is the same state, without asking the host
            if last.reservedAL == state.reservedAL || effect.are_param_states_identical(last, state).unwrap_or(true) || Some(*p) == reported {
                continue;
            }
            if set_to.get(p).is_some_and(|v| self.static_value(*p).is_some_and(|x| (x - v).abs() <= 1e-9 * v.abs().max(1.0))) {
                applied.push(*p);
            } else {
                changed.push(*p);
            }
        }

        let mut stored = self.stored.write();
        stored.keyframes_state = states;
        for p in applied {
            stored.keyframable_set_to.remove(&p);
        }
        changed
    }
    /// The value of the keyframable parameter `p`, `None` when it's keyframed
    fn static_value(&self, p: Params) -> Option<f64> {
        let def = match &self.inner {
            ParamsInner::Ae(x) => x.get(p).ok()?,
            ParamsInner::AeRO(x) => x.get(p).ok()?,
            ParamsInner::Premiere(_) => return None,
        };
        if def.keyframe_count().ok()? > 0 { return None; }
        Some(def.as_float_slider().ok()?.value())
    }
}
impl<'a, 'b> GyroflowPluginParams for ParamHandler<'a, 'b> {
    fn get_string(&self, p: Params) -> PluginResult<String> {
        if p == Params::InstanceId {
            return Ok(self.stored.read().instance_id.clone());
        }
        if let Some(v) = self.stored.read().pending_params_str.get(&p) {
            return Ok(v.clone());
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(ArbString::read(arb_handle(&*x.get(p)?)?))
            }
            ParamsInner::AeRO(x) => {
                Ok(ArbString::read(arb_handle(&*x.get(p)?)?))
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    // Flattened, as `ArbString` saves it: a `String`
                    Ok(filter.param_arbitrary_data::<String>(ind, render_params.clip_time()).map_err(|e| format!("{p:?}: {e:?}"))?)
                } else {
                    Ok(String::new())
                }
            }
        }
    }
    fn set_string(&mut self, p: Params, v: &str) -> PluginResult<()> {
        if p == Params::InstanceId {
            self.stored.write().instance_id = v.to_owned();
        }
        self.stored.write().pending_params_str.insert(p, v.to_owned());
        if p == Params::LoadedProject || p == Params::LoadedPreset || p == Params::LoadedLens { return Ok(()); }
        match &mut self.inner {
            // In the parameters the host gives the command, and keeps. Not in a copy it checks out for a render, which
            // it doesn't keep, and whose value is the host's own: the pending value is set with the next edit
            ParamsInner::Ae(x) if !x.raw_params().is_empty() => {
                let mut def = x.get_mut(p)?;
                arb_handle(&def)?;
                // Every change is a new value for the host to keep, and the project data can be megabytes
                if ArbString::write(unsafe { &mut def.as_mut().u.arb_d.value }, v)? {
                    def.set_value_changed();
                }
            }
            ParamsInner::Ae(_) => { }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }

        Ok(())
    }
    fn get_bool(&self, p: Params) -> PluginResult<bool> {
        if p == Params::Status {
            return Ok(self.get_string(p)? == "OK");
        }
        if let Some(v) = self.stored.read().pending_params_bool.get(&p) {
            return Ok(*v);
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_checkbox()?.value())
            }
            ParamsInner::AeRO(x) => {
                Ok(x.get(p)?.as_checkbox()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    match filter.param(ind, render_params.clip_time()) {
                        Ok(pr::Param::Bool(x)) => Ok(x),
                        _ => Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
        }
    }
    fn set_bool(&mut self, p: Params, v: bool) -> PluginResult<()> {
        if p == Params::Status {
            return Ok(());
        }
        self.stored.write().pending_params_bool.insert(p, v);
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_checkbox_mut()?.set_value(v);
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn get_f64(&self, p: Params) -> PluginResult<f64> {
        if p == Params::VideoSpeed { return Ok(100.0); }
        if p == Params::OutputWidth || p == Params::OutputHeight {
            let stored = self.stored.read();
            if stored.sequence_size != (0, 0) {
                return Ok(if p == Params::OutputWidth { stored.sequence_size.0 as f64 } else { stored.sequence_size.1 as f64 });
            }
        }
        if let Some(v) = self.stored.read().pending_params_f64.get(&p) {
            return Ok(*v);
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_float_slider()?.value())
            }
            ParamsInner::AeRO(x) => {
                Ok(x.get(p)?.as_float_slider()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    match filter.param(ind, render_params.clip_time()) {
                        Ok(pr::Param::Float64(x)) => Ok(x),
                        _ => Err("Param not found".into())
                    }
                } else {
                    Err("Param not found".into())
                }
            }
        }
    }
    fn set_f64(&mut self, p: Params, v: f64) -> PluginResult<()> {
        if p == Params::VideoSpeed { return Ok(()); }
        {
            let mut stored = self.stored.write();
            stored.pending_params_f64.insert(p, v);
            // The plugin's change, not news to it when the host applies it (see `changed_keyframable_params`)
            if matches!(self.inner, ParamsInner::Ae(_)) && is_keyframable(p) {
                stored.keyframable_set_to.insert(p, v);
            }
        }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_float_slider_mut()?.set_value(v);
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn get_i32(&self, p: Params) -> PluginResult<i32> {
        if let Some(v) = self.stored.read().pending_params_i32.get(&p) {
            return Ok(*v);
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_popup()?.value() - 1)
            }
            ParamsInner::AeRO(x) => {
                Ok(x.get(p)?.as_popup()?.value() - 1)
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    match filter.param(ind, render_params.clip_time()) {
                        Ok(pr::Param::Int32(x)) => Ok(x - 1),
                        _ => Err("Param not found".into())
                    }
                } else {
                    Err("Param not found".into())
                }
            }
        }
    }
    fn set_i32(&mut self, p: Params, v: i32) -> PluginResult<()> {
        self.stored.write().pending_params_i32.insert(p, v);
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_popup_mut()?.set_value(v + 1);
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn set_label(&mut self, p: Params, label: &str) -> PluginResult<()> {
        if p == Params::Status {
            self.set_string(p, label)?;
            return Ok(());
        }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                let mut x = x.get_mut(p)?.clone();
                if p == Params::OpenGyroflow {
                    x.as_button_mut()?.set_label(label);
                } else {
                    x.set_name(label)?;
                }
                x.update_param_ui()?;
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }

        Ok(())
    }
    fn set_hint(&mut self, _p: Params, _hint: &str) -> PluginResult<()> {
        Ok(())
    }
    fn set_enabled(&mut self, p: Params, v: bool) -> PluginResult<()> {
        if p == Params::VideoSpeed { return Ok(()); }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                let mut x = x.get_mut(p)?.clone();
                x.set_ui_flag(ae::ParamUIFlags::DISABLED, !v);
                x.set_flag(ae::ParamFlag::TWIRLY, true);
                x.update_param_ui()?;
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn get_f64_at_time(&self, p: Params, time: TimeType) -> PluginResult<f64> {
        if p == Params::VideoSpeed {
            let stored = self.stored.read();
            if self.get_bool(Params::StabilizationSpeedRamp).unwrap_or_default() && !stored.speed_per_frame.is_empty() {
                let frame = match time {
                    TimeType::Frame(x) => x as usize,
                    TimeType::FrameOrMicrosecond((Some(x), _)) => x as usize,
                    _ => panic!("Shouldn't happen"),
                };
                if let Some(v) = stored.speed_per_frame.get(frame) {
                    return Ok(*v);
                } else {
                    return Ok(*stored.speed_per_frame.last().unwrap());
                }
            } else {
                return Ok(100.0);
            }
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                let in_data = x.in_data();
                let ae_time = ae_time_from_timetype(time, in_data.time_step(), in_data.time_scale());
                Ok(x.get_at(p, Some(ae_time), Some(in_data.time_step()), Some(in_data.time_scale()))?.as_float_slider()?.value())
            }
            ParamsInner::AeRO(x) => {
                let in_data = x.in_data();
                let ae_time = ae_time_from_timetype(time, in_data.time_step(), in_data.time_scale());
                Ok(x.get_at(p, Some(ae_time), Some(in_data.time_step()), Some(in_data.time_scale()))?.as_float_slider()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    match filter.param(ind, ticks_from_timetype(time, render_params.render_ticks_per_frame())) {
                        Ok(pr::Param::Float64(x)) => Ok(x),
                        _ => Err("Param not found".into())
                    }
                } else {
                    Err("Param not found".into())
                }
            }
        }
    }
    fn get_bool_at_time(&self, p: Params, time: TimeType) -> PluginResult<bool> {
        match &self.inner {
            ParamsInner::Ae(x) => {
                let in_data = x.in_data();
                let ae_time = ae_time_from_timetype(time, in_data.time_step(), in_data.time_scale());
                Ok(x.get_at(p, Some(ae_time), Some(in_data.time_step()), Some(in_data.time_scale()))?.as_checkbox()?.value())
            }
            ParamsInner::AeRO(x) => {
                let in_data = x.in_data();
                let ae_time = ae_time_from_timetype(time, in_data.time_step(), in_data.time_scale());
                Ok(x.get_at(p, Some(ae_time), Some(in_data.time_step()), Some(in_data.time_scale()))?.as_checkbox()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                match filter.param(ind, ticks_from_timetype(time, render_params.render_ticks_per_frame())) {
                        Ok(pr::Param::Bool(x)) => Ok(x),
                        _ => Ok(false)
                    }
                } else {
                    Ok(false)
                }
            }
        }
    }
    fn clear_keyframes(&mut self, _param: Params) -> PluginResult<()> {
        Ok(())
    }
    fn is_keyframed(&self, p: Params) -> bool {
        if p == Params::VideoSpeed {
            return self.get_bool(Params::StabilizationSpeedRamp).unwrap_or_default() && !self.stored.read().speed_per_frame.is_empty();
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                if let Ok(keyframe_count) = x.get(p).and_then(|x| x.keyframe_count()) {
                    keyframe_count > 0
                } else {
                    self.stored.read().premiere_keyframed_params.contains(&p)
                }
            }
            ParamsInner::AeRO(x) => {
                if let Ok(keyframe_count) = x.get(p).and_then(|x| x.keyframe_count()) {
                    keyframe_count > 0
                } else {
                    self.stored.read().premiere_keyframed_params.contains(&p)
                }
            }
            ParamsInner::Premiere((filter, _render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    filter.next_keyframe_time(ind, -1) != Err(pr::Error::NoKeyframeAfterInTime)
                } else {
                    false
                }
            }
        }
    }
    fn get_keyframes(&self, _p: Params) -> Vec<(TimeType, f64)> {
        Vec::new()
    }
    fn set_f64_at_time(&mut self, _p: Params, _time: TimeType, _v: f64) -> PluginResult<()> {
        // TODO
        /*if p == Params::VideoSpeed { return Ok(()); }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_float_slider_mut()?.set_value(v);
            }
            ParamsInner::AeRO(_) => { }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }*/
        Ok(())
    }
}
