
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

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
#[repr(C)]
pub struct ArbString {
    pub val: String,
}
impl ArbString {
    pub fn get(&self) -> &str {
        self.val.as_str()
    }
    pub fn set(&mut self, v: &str) {
        self.val.clear();
        self.val.push_str(v);
    }
}

impl ae::ArbitraryData<ArbString> for ArbString {
    fn interpolate(&self, _other: &Self, _value: f64) -> Self {
        self.clone()
    }
}

pub fn define_param(params: &mut ae::Parameters<Params>, x: ParameterType, _group: Option<&'static str>) {
    match x {
        ParameterType::HiddenString { id } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, id, ae::ArbitraryDef::setup(|f| {
                f.set_default::<ArbString>(ArbString::default()).unwrap();
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::NO_ECW_UI);
                -1
            }).unwrap();
        }
        ParameterType::TextBox { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, label, ae::ArbitraryDef::setup(|f| {
                f.set_default::<ArbString>(ArbString::default()).unwrap();
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
                f.set_default::<ArbString>(ArbString::default()).unwrap();
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
                Ok(x.get(p)?.as_arbitrary()?.value::<ArbString>()?.get().to_string())
            }
            ParamsInner::AeRO(x) => {
                Ok(x.get(p)?.as_arbitrary()?.value::<ArbString>()?.get().to_string())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                if let Some(ind) = param_index_for_type(p, None) {
                    Ok(filter.param_arbitrary_data::<ArbString>(ind, render_params.clip_time()).unwrap().get().to_owned())
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
            ParamsInner::Ae(x) => {
                let mut p = x.get_mut(p)?;
                if let Ok(mut arb) = p.as_arbitrary_mut()?.value::<ArbString>() {
                    arb.set(v);
                }
                p.set_value_changed();
                // p.update_param_ui()?;
            }
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
        self.stored.write().pending_params_f64.insert(p, v);
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
                    x.set_name(label);
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
