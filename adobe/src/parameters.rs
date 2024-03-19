
use gyroflow_plugin_base::*;
use after_effects as ae;
use ae::{ ParamFlag, ParamUIFlags, ValueDisplayFlag };
use std::str::FromStr;
use serde::{ Serialize, Serializer, Deserialize, Deserializer };
use std::sync::Arc;
use parking_lot::RwLock;

pub fn frame_from_timetype(time: TimeType) -> f64 {
    match time {
        TimeType::Frame(x) => x,
        TimeType::FrameOrMicrosecond((Some(x), _)) => x,
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

/*pub fn ser_str<S: Serializer>(x: &arrayvec::ArrayString<512>, s: S) -> Result<S::Ok, S::Error> {
    Serialize::serialize(x.as_str(), s)
}
pub fn de_str<'de, D: Deserializer<'de>>(d: D) -> Result<arrayvec::ArrayString<512>, D::Error> {
    let strs: String = Deserialize::deserialize(d)?;
    Ok(arrayvec::ArrayString::<512>::from(&strs).unwrap())
}*/

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
//pub struct ArbString(pub String);
#[repr(C)]
pub struct ArbString {
    //#[serde(serialize_with="ser_str", deserialize_with="de_str")]
    //pub arr: arrayvec::ArrayString<512>,
    pub val: String,
}
impl ArbString {
    /*pub fn new(v: &str) -> Self {
        Self {
            arr: arrayvec::ArrayString::<256>::from(v).unwrap(),
            id: ae::fastrand::u32(..),
        }
    }*/
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
                //f.set_refcon(1 as _);
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::NO_ECW_UI);
                -1
            }).unwrap();
        }
        ParameterType::TextBox      { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, label, ae::ArbitraryDef::setup(|f| {
                f.set_default::<ArbString>(ArbString::default()).unwrap();
                //f.set_refcon(1 as _);
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::CONTROL);
                param.set_ui_width(250);
                param.set_ui_height(20);
                -1
            }).unwrap();
        }

        ParameterType::Button { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            if p == Params::OpenGyroflow { return; }
            params.add_with_flags(p, "", ae::ButtonDef::setup(|f| { f.set_label(label); }), ParamFlag::SUPERVISE | ParamFlag::CANNOT_TIME_VARY, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Text { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            params.add_customized(p, label, ae::ArbitraryDef::setup(|f| {
                f.set_default::<ArbString>(ArbString::default()).unwrap();
                //f.set_refcon(1 as _);
            }), |param| {
                param.set_flags(ae::ParamFlag::CANNOT_TIME_VARY);
                param.set_ui_flags(ae::ParamUIFlags::CONTROL);
                param.set_ui_width(250);
                param.set_ui_height(20);
                -1
            }).unwrap();
            /*if id == "Status" { return; }
            params.add_with_flags(Params::from_str(id).unwrap(), label, ae::CheckBoxDef::setup(|f| {
                f.set_default(false);
                f.set_value(false);
                f.set_label("");
            }), ParamFlag::SUPERVISE, ParamUIFlags::DISABLED).unwrap();*/
        }
        ParameterType::Slider { id, label, min, max, default, .. } => {
            params.add_with_flags(Params::from_str(id).unwrap(), label, ae::FloatSliderDef::setup(|f| {
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
            params.add_with_flags(Params::from_str(id).unwrap(), label, ae::CheckBoxDef::setup(|f| {
                f.set_default(default);
                f.set_value(default);
                f.set_label("ON");
            }), ParamFlag::SUPERVISE, ParamUIFlags::empty()).unwrap();
        }
        ParameterType::Group { id, label, parameters } => {
            params.add_group(Params::from_str(id).unwrap(), Params::from_str(&format!("{id}End")).unwrap(), label, |params| {
                for x in parameters {
                    define_param(params, x, Some(id));
                }
            }).unwrap();
        }
    }
}

pub fn param_index_for_type(type_: Params, init: Option<std::collections::HashMap<Params, ae::ParamMapInfo>>) -> Option<usize> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<Params, ae::ParamMapInfo>> = std::sync::OnceLock::new();
    let map = MAP.get_or_init(|| init.unwrap());

    map.get(&type_).map(|x| x.index)
}


use gyroflow_plugin_base::PluginResult;

use crate::StoredParams;

pub struct ParamHandler<'a, 'b> where 'b: 'a {
    pub inner: &'a mut ae::Parameters<'b, Params>,
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
        Ok(self.inner.get(p)?.as_arbitrary()?.value::<ArbString>()?.get().to_string())
    }
    fn set_string(&mut self, p: Params, v: &str) -> PluginResult<()> {
        log::info!("set_string: {p:?} = {v}");
        if p == Params::Status {
            self.stored.write().status = v.to_owned();
        }
        if p == Params::InstanceId {
            self.stored.write().instance_id = v.to_owned();
        }
        let mut p = self.inner.get_mut(p)?;
        p.as_arbitrary_mut()?.value::<ArbString>()?.set(v);
        p.set_value_changed();
        p.update_param_ui()?;

        Ok(())
    }
    fn get_bool(&self, p: Params) -> PluginResult<bool> {
        if p == Params::Status {
            return Ok(self.get_string(p)? == "OK");
        }
        Ok(self.inner.get(p)?.as_checkbox()?.value())
    }
    fn set_bool(&mut self, p: Params, v: bool) -> PluginResult<()> {
        if p == Params::Status {
            if v {
                self.set_string(p, "OK")?;
            } else {
                self.set_string(p, "ERR")?;
            }
            return Ok(());
        }
        self.inner.get_mut(p)?.as_checkbox_mut()?.set_value(v);
        Ok(())
    }
    fn get_f64(&self, p: Params) -> PluginResult<f64> {
        Ok(self.inner.get(p)?.as_float_slider()?.value())
    }
    fn set_f64(&mut self, p: Params, v: f64) -> PluginResult<()> {
        self.inner.get_mut(p)?.as_float_slider_mut()?.set_value(v);
        Ok(())
    }
    fn set_label(&mut self, p: Params, label: &str) -> PluginResult<()> {
        if p == Params::Status {
            self.set_string(p, label)?;
            return Ok(());
        }
        let mut x = self.inner.get_mut(p)?.clone();
        x.set_name(label);
        x.update_param_ui()?;

        Ok(())
    }
    fn set_hint(&mut self, _p: Params, _hint: &str) -> PluginResult<()> {
        Ok(())
    }
    fn set_enabled(&mut self, p: Params, v: bool) -> PluginResult<()> {
        let mut x = self.inner.get_mut(p)?.clone();
        x.set_ui_flag(ae::ParamUIFlags::DISABLED, !v);
        x.set_flag(ae::ParamFlag::TWIRLY, true);
        x.update_param_ui()?;
        Ok(())
    }
    fn get_f64_at_time(&self, p: Params, time: TimeType) -> PluginResult<f64> {
        // TODO
        Ok(self.inner.get(p)?.as_float_slider()?.value())
    }
    fn get_bool_at_time(&self, p: Params, time: TimeType) -> PluginResult<bool> {
        // TODO
        Ok(self.inner.get(p)?.as_checkbox()?.value())
    }
    fn clear_keyframes(&mut self, param: Params) -> PluginResult<()> {
        // TODO
        Ok(())
    }
    fn is_keyframed(&self, p: Params) -> bool {
        self.inner
            .get(p)
            .map(|x| x.keyframe_count().unwrap_or(0) > 0)
            .unwrap_or_default()
    }
    fn get_keyframes(&self, p: Params) -> Vec<(TimeType, f64)> {
        // TODO
        Vec::new()
    }
    fn set_f64_at_time(&mut self, p: Params, time: TimeType, v: f64) -> PluginResult<()> {
        // TODO
        self.inner.get_mut(p)?.as_float_slider_mut()?.set_value(v);
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