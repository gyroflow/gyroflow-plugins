
use gyroflow_plugin_base::*;
use after_effects as ae;
use ae::{ ParamFlag, ParamUIFlags, ValueDisplayFlag };
use std::str::FromStr;
use serde::{ Serialize, Serializer, Deserialize, Deserializer };

pub fn frame_from_timetype(time: TimeType) -> f64 {
    match time {
        TimeType::Frame(x) => x,
        TimeType::FrameOrMicrosecond((Some(x), _)) => x,
        _ => panic!("Shouldn't happen"),
    }
}

define_params!(ParamHandler {
    strings: [
        InstanceId          => instance_id:      Params,
        ProjectData         => project_data:     Params,
        EmbeddedLensProfile => embedded_lens:    Params,
        EmbeddedPreset      => embedded_preset:  Params,
        ProjectPath         => project_path:     Params,
        OpenGyroflow        => open_in_gyroflow: Params,
        ReloadProject       => reload_project:   Params,
    ],
    bools: [
        DisableStretch        => disable_stretch:         Params,
        Status                => status:                  Params,
        ToggleOverview        => toggle_overview:         Params,
        DontDrawOutside       => dont_draw_outside:       Params,
        IncludeProjectData    => include_project_data:    Params,
        UseGyroflowsKeyframes => use_gyroflows_keyframes: Params,
    ],
    f64s: [
        InputRotation         => input_rotation:           Params,
        Fov                   => fov:                      Params,
        Smoothness            => smoothness:               Params,
        LensCorrectionStrength=> lens_correction_strength: Params,
        HorizonLockAmount     => horizon_lock_amount:      Params,
        HorizonLockRoll       => horizon_lock_roll:        Params,
        PositionX             => positionx:                Params,
        PositionY             => positiony:                Params,
        Rotation              => rotation:                 Params,
        VideoSpeed            => video_speed:              Params,
    ],

    get_string: s p {
        Ok(s.stored.get_string(*p).to_owned())
    },
    set_string: s p, v {
        s.stored.set_string(*p, v);
        Ok(())
    },
    get_bool: s p {
        let v = s.params.get_checkbox(*p, None, None, None)
                        .map(|x| x.value())
                        .unwrap_or_default();
        log::info!("get_bool: {p:?} => {v}");
        Ok(v)
    },
    set_bool: s p, v {
        s.params.get_checkbox(*p, None, None, None)
                .map(|x| x.set_value(if v { 1 } else { 0 }));

        s.params.get_param_def_mut(*p, None, None, None)
                .map(|mut x| { x.set_value_has_changed(); });
        Ok(())
    },
    get_f64: s p {
        let v = s.params.get_float_slider(*p, None, None, None)
                        .map(|x| x.value())
                        .unwrap_or_default();
        log::info!("get_f64: {p:?} => {v}, is_none: {}", s.params.get_float_slider(*p, None, None, None).is_none());
        Ok(v)
    },
    set_f64: s p, v {
        s.params.get_float_slider(*p, None, None, None)
                .map(|x| x.set_value(v));

        s.params.get_param_def_mut(*p, None, None, None)
                .map(|mut x| { x.set_value_has_changed(); });
        Ok(())
    },
    set_label: s p, l {
        s.params.get_param_def_mut(*p, None, None, None)
                .map(|mut x| {
                    log::info!("set_label: {p:?} => {l}");
                    x.name(l);
                    //x.set_value_has_changed();
                    x.update_param_ui();
                });
        Ok(())
    },
    set_hint: _s _p, _h { Ok(()) },
    set_enabled: s p, e {
        s.params.get_param_def_mut(*p, None, None, None)
                .map(|mut x| {
                    let mut flags = x.get_ui_flags();
                    flags.set(ParamUIFlags::DISABLED, !e);
                    x.ui_flags(flags);
                    //x.set_value_has_changed();
                    x.update_param_ui();
                });
        Ok(())
    },
    get_bool_at_time: s p, t {
        let in_data = s.params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        Ok(s.params.get_checkbox(*p, Some(time as i32), None, None)
                   .map(|x| x.value())
                   .unwrap_or_default())
    },
    get_f64_at_time: s p, t {
        let in_data = s.params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        Ok(s.params.get_float_slider(*p, Some(time as i32), None, None)
                   .map(|x| x.value())
                   .unwrap_or_default())
    },
    set_f64_at_time: s p, t, v {
        let in_data = s.params.in_data();
        let time = frame_from_timetype(t) * in_data.time_step() as f64;
        s.params.get_float_slider(*p, Some(time as i32), None, None)
                .map(|x| x.set_value(v));
        s.params.get_param_def_mut(*p, Some(time as i32), None, None)
                .map(|mut x| { x.set_value_has_changed(); });
        Ok(())
    },
    is_keyframed: s p {
        s.params.get_param_def(*p, None, None, None)
                .map(|x| x.keyframe_count() > 0).unwrap_or_default()
    },
    get_keyframes: _s _p {
        Vec::new()
    },
    clear_keyframes: _s _p {
        Ok(())
    },

    params: ae::Parameters<Params>,
    stored: StoredParams,
});
unsafe impl Send for ParamHandler { }
unsafe impl Sync for ParamHandler { }

impl Default for ParamHandler {
    fn default() -> Self {
        Self {
            instance_id:              Params::InstanceId,
            project_data:             Params::ProjectData,
            embedded_lens:            Params::EmbeddedLensProfile,
            embedded_preset:          Params::EmbeddedPreset,
            project_path:             Params::ProjectPath,
            disable_stretch:          Params::DisableStretch,
            status:                   Params::Status,
            open_in_gyroflow:         Params::OpenGyroflow,
            reload_project:           Params::ReloadProject,
            toggle_overview:          Params::ToggleOverview,
            dont_draw_outside:        Params::DontDrawOutside,
            include_project_data:     Params::IncludeProjectData,
            input_rotation:           Params::InputRotation,
            use_gyroflows_keyframes:  Params::UseGyroflowsKeyframes,
            fov:                      Params::Fov,
            smoothness:               Params::Smoothness,
            lens_correction_strength: Params::LensCorrectionStrength,
            horizon_lock_amount:      Params::HorizonLockAmount,
            horizon_lock_roll:        Params::HorizonLockRoll,
            video_speed:              Params::VideoSpeed,
            positionx:                Params::PositionX,
            positiony:                Params::PositionY,
            rotation:                 Params::Rotation,

            fields:                   Default::default(),
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub(crate) struct StoredParams {
    pub(crate) instance_id: String,
    pub(crate) project_data: String,
    pub(crate) embedded_lens: String,
    pub(crate) embedded_preset: String,
    pub(crate) project_path: String,
}
impl StoredParams {
    pub(crate) fn set_string(&mut self, p: Params, v: &str) {
        match p {
            Params::InstanceId          => self.instance_id = v.to_string(),
            Params::ProjectData         => self.project_data = v.to_string(),
            Params::EmbeddedLensProfile => self.embedded_lens = v.to_string(),
            Params::EmbeddedPreset      => self.embedded_preset = v.to_string(),
            Params::ProjectPath         => self.project_path = v.to_string(),
            _ => panic!("Unexpected string {p:?}")
        }
    }
    pub(crate) fn get_string(&self, p: Params) -> &str {
        match p {
            Params::InstanceId          => &self.instance_id,
            Params::ProjectData         => &self.project_data,
            Params::EmbeddedLensProfile => &self.embedded_lens,
            Params::EmbeddedPreset      => &self.embedded_preset,
            Params::ProjectPath         => &self.project_path,
            _ => panic!("Unexpected string {p:?}")
        }
    }
}
pub fn ser_stored<S: Serializer>(x: &GyroflowPluginBaseInstance<ParamHandler>, s: S) -> Result<S::Ok, S::Error> {
    Serialize::serialize(&x.parameters.fields.stored, s)
}
pub fn de_stored<'de, D: Deserializer<'de>>(d: D) -> Result<GyroflowPluginBaseInstance<ParamHandler>, D::Error> {
    let strs: StoredParams = Deserialize::deserialize(d)?;
    let mut inst = crate::Instance::new_base_instance();
    inst.parameters.fields.stored = strs;
    Ok(inst)
}

pub fn define_param(params: &mut ae::Parameters<Params>, x: ParameterType, _group: Option<&'static str>) {
    match x {
        ParameterType::HiddenString { .. } => { }
        ParameterType::TextBox      { .. } => { }

        ParameterType::Button { id, label, .. } => {
            let p = Params::from_str(id).unwrap();
            if p == Params::LoadCurrent { return; }
            if p == Params::OpenGyroflow { return; }
            params.add_param_with_flags(p, "", ae::ButtonDef::new().label(label), ParamFlag::SUPERVISE | ParamFlag::CANNOT_TIME_VARY, ParamUIFlags::empty());
        }
        ParameterType::Text { id, label, .. } => {
            params.add_param_with_flags(Params::from_str(id).unwrap(), label, ae::CheckBoxDef::new()
                .set_default(false)
                .set_value(0)
                .label("")
            , ParamFlag::SUPERVISE, ParamUIFlags::DISABLED);
        }
        ParameterType::Slider { id, label, min, max, default, .. } => {
            params.add_param_with_flags(Params::from_str(id).unwrap(), label, ae::FloatSliderDef::new()
                .set_valid_min(min as f32)
                .set_slider_min(min as f32)
                .set_valid_max(max as f32)
                .set_slider_max(max as f32)
                .set_value(default)
                .set_default(default as f32)
                .precision(1)
                .display_flags(ValueDisplayFlag::NONE)
            , ParamFlag::SUPERVISE, ParamUIFlags::empty());
        }
        ParameterType::Checkbox { id, label, default, .. } => {
            params.add_param_with_flags(Params::from_str(id).unwrap(), label, ae::CheckBoxDef::new()
                .set_default(default)
                .set_value(if default { 1 } else { 0 })
                .label("ON")
            , ParamFlag::SUPERVISE, ParamUIFlags::empty());
        }
        ParameterType::Group { id, label, parameters } => {
            params.add_group(Params::from_str(id).unwrap(), Params::from_str(&format!("{id}End")).unwrap(), label, |params| {
                for x in parameters {
                    define_param(params, x, Some(id));
                }
            });
        }
    }
}
