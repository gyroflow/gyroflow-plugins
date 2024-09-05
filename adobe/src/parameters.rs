
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
                param.set_ui_height(15);
                -1
            }).unwrap();
        }
        ParameterType::Slider { id, label, min, max, default, .. } => {
            let p = Params::from_str(id).unwrap();
            //if p == Params::VideoSpeed { return; }
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
        ParameterType::Group { id, label, parameters, opened } => {
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
    Premiere((&'a pr::GpuFilterData, pr::RenderParams))
}

pub struct ParamHandler<'a, 'b> where 'b: 'a {
    pub inner: ParamsInner<'a, 'b>,
    pub stored: Arc<RwLock<StoredParams>>,
}
impl<'a, 'b> GyroflowPluginParams for ParamHandler<'a, 'b> {
    fn get_string(&self, p: Params) -> PluginResult<String> {
        if p == Params::ProjectPath && !self.stored.read().project_path.is_empty() {
            return Ok(self.stored.read().project_path.clone());
        }
        if p == Params::Status {
            return Ok(self.stored.read().status.clone());
        }
        if p == Params::InstanceId {
            return Ok(self.stored.read().instance_id.clone());
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_arbitrary()?.value::<ArbString>()?.get().to_string())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                Ok(filter.param_arbitrary_data::<ArbString>(param_index_for_type(p, None).unwrap(), render_params.clip_time()).unwrap().get().to_owned())
            }
        }
    }
    fn set_string(&mut self, p: Params, v: &str) -> PluginResult<()> {
        log::info!("set_string: {p:?} = {v}");
        if p == Params::ProjectPath && matches!(self.inner, ParamsInner::Ae(_)) {
            // let arbitrary data take over
            self.stored.write().project_path.clear();
        }
        if p == Params::Status {
            self.stored.write().status = v.to_owned();
        }
        if p == Params::InstanceId {
            self.stored.write().instance_id = v.to_owned();
        }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                let mut p = x.get_mut(p)?;
                if let Ok(mut arb) = p.as_arbitrary_mut()?.value::<ArbString>() {
                    arb.set(v);
                }
                p.set_value_changed();
                // p.update_param_ui()?;
            }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }

        Ok(())
    }
    fn get_bool(&self, p: Params) -> PluginResult<bool> {
        if p == Params::Status {
            return Ok(self.get_string(p)? == "OK");
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_checkbox()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                match filter.param(param_index_for_type(p, None).unwrap(), render_params.clip_time()) {
                    Ok(pr::Param::Bool(x)) => Ok(x),
                    _ => Ok(false)
                }
            }
        }
    }
    fn set_bool(&mut self, p: Params, v: bool) -> PluginResult<()> {
        if p == Params::Status {
            return Ok(());
        }
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_checkbox_mut()?.set_value(v);
            }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn get_f64(&self, p: Params) -> PluginResult<f64> {
        if p == Params::OutputWidth || p == Params::OutputHeight {
            let stored = self.stored.read();
            if stored.sequence_size != (0, 0) {
                return Ok(if p == Params::OutputWidth { stored.sequence_size.0 as f64 } else { stored.sequence_size.1 as f64 });
            }
        }
        match &self.inner {
            ParamsInner::Ae(x) => {
                Ok(x.get(p)?.as_float_slider()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                match filter.param(param_index_for_type(p, None).unwrap(), render_params.clip_time()) {
                    Ok(pr::Param::Float64(x)) => Ok(x),
                    _ => Err("Param not found".into())
                }
            }
        }
    }
    fn set_f64(&mut self, p: Params, v: f64) -> PluginResult<()> {
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_float_slider_mut()?.set_value(v);
            }
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
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }

        Ok(())
    }
    fn set_hint(&mut self, _p: Params, _hint: &str) -> PluginResult<()> {
        Ok(())
    }
    fn set_enabled(&mut self, p: Params, v: bool) -> PluginResult<()> {
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                let mut x = x.get_mut(p)?.clone();
                x.set_ui_flag(ae::ParamUIFlags::DISABLED, !v);
                x.set_flag(ae::ParamFlag::TWIRLY, true);
                x.update_param_ui()?;
            }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
    fn get_f64_at_time(&self, p: Params, time: TimeType) -> PluginResult<f64> {
        match &self.inner {
            ParamsInner::Ae(x) => {
                let in_data = x.in_data();
                let ae_time = ae_time_from_timetype(time, in_data.time_step(), in_data.time_scale());
                Ok(x.get_at(p, Some(ae_time), Some(in_data.time_step()), Some(in_data.time_scale()))?.as_float_slider()?.value())
            }
            ParamsInner::Premiere((filter, render_params)) => {
                match filter.param(param_index_for_type(p, None).unwrap(), ticks_from_timetype(time, render_params.render_ticks_per_frame())) {
                    Ok(pr::Param::Float64(x)) => Ok(x),
                    _ => Err("Param not found".into())
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
            ParamsInner::Premiere((filter, render_params)) => {
                match filter.param(param_index_for_type(p, None).unwrap(), ticks_from_timetype(time, render_params.render_ticks_per_frame())) {
                    Ok(pr::Param::Bool(x)) => Ok(x),
                    _ => Ok(false)
                }
            }
        }
    }
    fn clear_keyframes(&mut self, param: Params) -> PluginResult<()> {
        // TODO
        Ok(())
    }
    fn is_keyframed(&self, p: Params) -> bool {
        match &self.inner {
            ParamsInner::Ae(x) => {
                if let Ok(keyframe_count) = x.get(p).and_then(|x| x.keyframe_count()) {
                    keyframe_count > 0
                } else {
                    // I didn't find any way to check if a param is keyframed in Premiere, so let's assume they are all keyframed
                    // TODO
                    true
                }
            }
            ParamsInner::Premiere((filter, _render_params)) => {
                filter.next_keyframe_time(param_index_for_type(p, None).unwrap(), -1) != Err(pr::Error::NoKeyframeAfterInTime)
            }
        }
    }
    fn get_keyframes(&self, _p: Params) -> Vec<(TimeType, f64)> {
        Vec::new()
    }
    fn set_f64_at_time(&mut self, p: Params, time: TimeType, v: f64) -> PluginResult<()> {
        // TODO
        match &mut self.inner {
            ParamsInner::Ae(x) => {
                x.get_mut(p)?.as_float_slider_mut()?.set_value(v);
            }
            ParamsInner::Premiere(_) => { } // Premiere can't set param values
        }
        Ok(())
    }
}

/*
    get_bool_at_time: s p, t {
        let params = s.params()?;
        let in_data = params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        Ok(params.get_at(*p, Some(time as i32), None, None)?.as_checkbox()?.value())
    },
    get_f64_at_time: s p, t {
        let params = s.params()?;
        let in_data = params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        Ok(params.get_at(*p, Some(time as i32), None, None)?.as_float_slider()?.value())
    },
    set_f64_at_time: s p, t, v {
        let mut params = s.params()?;
        let in_data = params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        params.get_mut_at(*p, Some(time as i32), None, None)?.as_float_slider_mut()?.set_value(v);
        Ok(())
    },
    is_keyframed: s p {
        s.params()
            .and_then(|x| x.get(*p))
            .map(|x| x.keyframe_count().unwrap_or(0) > 0).unwrap_or_default()
    },
    get_keyframes: _s _p {
        Vec::new()
    },
*/