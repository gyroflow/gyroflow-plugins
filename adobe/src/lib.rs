
use after_effects as ae;

use gyroflow_plugin_base::*;
use gyroflow_plugin_base::gyroflow_core::GyroflowCoreError;
use std::collections::HashSet;
use lru::LruCache;
use parking_lot::RwLock;
use std::sync::Arc;
use ae::aegp::{ LayerFlags, LayerStream, TimeMode, PluginId, StreamValue };

mod ui;
mod premiere;

mod parameters;
use parameters::*;

use serde::{ Serialize, Deserialize };

static mut AEGP_PLUGIN_ID: PluginId = 0;
static mut IS_PREMIERE: bool = false;
static GLOBAL_INST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
pub(crate) fn global_inst<'a>() -> &'a mut Plugin {
    unsafe {
        let ptr = (*GLOBAL_INST.get_or_init(|| 0)) as *mut Plugin;
        &mut *ptr
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
    pub speed_checksum: u64
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
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Instance {
    #[serde(serialize_with="ser_stored", deserialize_with="de_stored")]
    stored: Arc<RwLock<StoredParams>>,
    gyroflow: Option<GyroflowPluginBaseInstance>
}
ae::define_cross_thread_type!(Instance);

impl CrossThreadInstance {
    fn new_base_instance(instance_id: &mut String) -> GyroflowPluginBaseInstance {
        log::info!("new_base_instance: {:?}", instance_id);
        let mut gyroflow = GyroflowPluginBaseInstance {
            managers:                       LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
            original_output_size:           (0, 0),
            original_video_size:            (0, 0),
            timeline_size:                  (0, 0),
            num_frames:                     0,
            fps:                            0.0,
            has_motion:                     false,
            reload_values_from_project:     true,
            ever_changed:                   false,
            opencl_disabled:                false,
            cache_keyframes_every_frame:    true,
            framebuffer_inverted:           false, //unsafe { IS_PREMIERE },
            anamorphic_adjust_size:         false,
            always_set_input_rotation:      true,
            keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                use_gyroflows_keyframes:  false,
                cached_keyframes:         KeyframeManager::default()
            })),
        };
        gyroflow.initialize_instance_id(instance_id);
        gyroflow
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

impl Instance {
    fn stab_manager<'a, 'b>(&mut self, params: &'a mut ParamHandler<'a, 'b>, global: &mut Plugin, output_rect: ae::Rect) -> Option<Arc<StabilizationManager>> {
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
                                BufferSource::MetalBuffer { buffer: in_ptr  as *mut metal::MTLBuffer, command_queue: device_info.command_queuePV as *mut metal::MTLCommandQueue },
                                BufferSource::MetalBuffer { buffer: out_ptr as *mut metal::MTLBuffer, command_queue: std::ptr::null_mut() },
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

                    let input_rotation = {
                        let params = ParamHandler { inner: ParamsInner::AeRO(plugin.params), stored: stored.clone() };
                        -params.get_f64(Params::InputRotation).unwrap_or_default() as f32
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

    fn cpu_render(in_data: ae::InData, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        if let Some(stab) = in_data.frame_data::<RenderData>() {
            let RenderData { stab, .. } = stab;
            let timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

            let org_ratio = {
                let params = stab.params.read();
                params.size.0 as f64 / params.size.1 as f64
            };

            let src_size = (src.width() as usize, src.height() as usize, src.buffer_stride());
            let dst_size = (dst.width() as usize, dst.height() as usize, dst.buffer_stride());
            let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);

            // log::info!("org_ratio: {org_ratio:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dst_size: {dst_size:?}, src.stride: {}, bit_depth: {}", src.row_bytes(), src.bit_depth());

            let src_buffer = unsafe { std::slice::from_raw_parts_mut(src.buffer().as_ptr() as *mut u8, src.buffer().len()) };
            let dst_buffer = unsafe { std::slice::from_raw_parts_mut(dst.buffer().as_ptr() as *mut u8, dst.buffer().len()) };

            let mut buffers = Buffers {
                input:  BufferDescription { size: src_size, rect: Some(src_rect), data: BufferSource::Cpu { buffer: src_buffer }, rotation: None, texture_copy: false },
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
    fn can_load(_host_name: &str, _host_version: &str) -> bool {
        true
    }

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

    fn handle_command(&mut self, cmd: ae::Command, in_data: InData, mut out_data: OutData, _params: &mut ae::Parameters<Params>) -> Result<(), ae::Error> {
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
                        GLOBAL_INST.set(self as *mut _ as usize).unwrap();
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

                    if let Ok(util) = ae::aegp::suites::Utility::new() {
                        unsafe { AEGP_PLUGIN_ID = util.register_with_aegp(None, "Gyroflow")?; }
                    }
                }

                let _ = in_data.effect().set_options_button_name("Open Gyroflow");
            },
            ae::Command::ArbitraryCallback { mut extra } => {
                if let Err(e) = extra.dispatch::<ArbString, Params>(Params::InstanceId) {
                    log::info!("arb callback error, which: {:?}", extra.which_function());
                    return Err(e);
                }
                let _ = extra.dispatch::<ArbString, Params>(Params::ProjectData);
                let _ = extra.dispatch::<ArbString, Params>(Params::EmbeddedLensProfile);
                let _ = extra.dispatch::<ArbString, Params>(Params::EmbeddedPreset);
                let _ = extra.dispatch::<ArbString, Params>(Params::Status);
                let _ = extra.dispatch::<ArbString, Params>(Params::ProjectPath);
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
        match param {
            Params::Fov | Params::Smoothness | Params::ZoomLimit | Params::LensCorrectionStrength |
            Params::HorizonLockAmount | Params::HorizonLockRoll |
            Params::AdditionalPitch | Params::AdditionalYaw | Params::Rotation | Params::InputRotation | Params::VideoSpeed |
            Params::UseGyroflowsKeyframes | Params::RecalculateKeyframes |
            Params::OutputHeight | Params::OutputWidth | Params::OutputSizeSwap | Params::OutputSizeToTimeline => {
                let _self = self.get().unwrap();
                let _self = _self.read();
                if matches!(param, Params::OutputHeight | Params::OutputWidth | Params::OutputSizeSwap | Params::OutputSizeToTimeline) {
                    let mut stored = _self.stored.write();
                    stored.sequence_size = (0, 0);
                }
                let ever_changed = _self.gyroflow.as_ref().map(|x| x.ever_changed).unwrap_or_default();
                if !ever_changed {
                    log::warn!("Instance ID changed, creating new cross thread instance!");
                    let new = Self::default();
                    let new_inst = new.get().unwrap();
                    let mut new_inst = new_inst.write();
                    *new_inst.stored.write() = _self.stored.read().clone();
                    new_inst.stored.write().instance_id = format!("{}", fastrand::u64(..));
                    new_inst.gyroflow = _self.gyroflow.clone();
                    new_inst.gyroflow.as_mut().unwrap().ever_changed = true;

                    self.id = new.id;
                }
            }
            _ => { }
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
                    stored.pending_params_f64.clone(),
                    stored.pending_params_bool.clone(),
                    stored.pending_params_str.clone(),
                    stored.pending_params_i32.clone()
                )
            };
            let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: stored };
            for (k, v) in &pending_f64s    { if *k != param { let _ = params.set_f64(*k, *v); } }
            for (k, v) in &pending_bools   { if *k != param { let _ = params.set_bool(*k, *v); } }
            for (k, v) in &pending_strings { if *k != param { let _ = params.set_string(*k, v); } }
            for (k, v) in &pending_i32s    { if *k != param { let _ = params.set_i32(*k, *v); } }
        }

        let _self = self.get().unwrap();

        let mut _self = _self.write();

        _self.gyroflow.as_mut().unwrap().keyframable_params.write().use_gyroflows_keyframes = plugin.params.get(Params::UseGyroflowsKeyframes)?.as_checkbox()?.value();
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
            if let Err(e) = inst.param_changed(&mut params, &plugin.global.gyroflow.manager_cache, param, true) {
                log::error!("param_changed error: {e:?}");
            }

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
        Ok((1, bincode::serialize(&self).unwrap()))
    }
    fn unflatten(_version: u16, bytes: &[u8]) -> Result<Self, Error> {
        match bincode::deserialize::<Self>(bytes) {
            Ok(inst) => {
                let mut _self = inst.get().unwrap();
                let mut _self = _self.write();
                if _self.gyroflow.is_none() {
                    let gyroflow = {
                        let mut stored = _self.stored.write();
                        Self::new_base_instance(&mut stored.instance_id)
                    };
                    log::info!("_self.gyroflow is none: {} | instance_id: {}", _self.gyroflow.is_none(), _self.stored.read().instance_id);
                    _self.gyroflow = Some(gyroflow);
                }
                Ok(inst)
            },
            Err(_) => {
                Ok(Self::default())
            }
        }
    }

    fn do_dialog(&mut self, plugin: &mut PluginState) -> Result<(), ae::Error> {
        self.user_changed_param(plugin, Params::OpenGyroflow)
    }

    fn render(&self, plugin: &mut PluginState, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        Instance::cpu_render(plugin.in_data, src, dst)
    }

    fn handle_command(&mut self, plugin: &mut PluginState, cmd: ae::Command) -> Result<(), ae::Error> {
        // log::info!("sequence command: {:?}, thread: {:?}, ptr: {:?}", cmd, std::thread::current().id(), self as *const _);

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
                stored.sequence_size = (in_data.width() as _, in_data.height() as _);

                let mut footage_path = String::new();
                let _ = (|| -> Result<(), ae::Error> {
                    let layer = in_data.effect().layer()?;
                    let item = layer.source_item()?;
                    let mut pixel_aspect_ratio = 1.0f64;
                    if item.item_type()? == ae::aegp::ItemType::Footage {
                        if let Ok(par) = item.pixel_aspect_ratio() {
                            pixel_aspect_ratio = par.into();
                        }
                        footage_path = item.main_footage()?.path(0, 0)?;
                    } else if item.item_type()? == ae::aegp::ItemType::Comp {
                        let comp = item.composition()?;
                        for i in 0..comp.num_layers()? {
                            let item = comp.layer_by_index(i)?.source_item()?;
                            if item.item_type()? == ae::aegp::ItemType::Footage {
                                footage_path = item.main_footage()?.path(0, 0)?;
                                if let Ok(par) = item.pixel_aspect_ratio() {
                                    pixel_aspect_ratio = par.into();
                                }
                                break;
                            }
                        }
                    }
                    if (pixel_aspect_ratio * 100.0).round() != 100.0 {
                        stored.pending_params_bool.insert(Params::DisableStretch, true);
                    }

                    let comp_dimensions = layer.parent_comp()?.item()?.dimensions()?;
                    stored.sequence_size = (comp_dimensions.0 as _, comp_dimensions.1 as _);
                    Ok(())
                })();

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
                    let stored = _self.stored.clone();

                    let mut trim_range = None;

                    let _ = (|| -> Result<(), ae::Error> {
                        let layer = in_data.effect().layer()?;
                        let item = layer.source_item()?;
                        if item.item_type()? == ae::aegp::ItemType::Footage {
                            let in_point = layer.in_point(ae::aegp::TimeMode::LayerTime)?;
                            let out_point = in_point + layer.duration(ae::aegp::TimeMode::LayerTime)?;
                            trim_range = Some((in_point.into(), out_point.into()));
                        } else if item.item_type()? == ae::aegp::ItemType::Comp {
                            let comp = item.composition()?;
                            for i in 0..comp.num_layers()? {
                                let layer = comp.layer_by_index(i)?;
                                if layer.source_item()?.item_type()? == ae::aegp::ItemType::Footage {
                                    let in_point = layer.in_point(ae::aegp::TimeMode::LayerTime)?;
                                    let out_point = in_point + layer.duration(ae::aegp::TimeMode::LayerTime)?;
                                    trim_range = Some((in_point.into(), out_point.into()));
                                    break;
                                }
                            }
                        }
                        Ok(())
                    })();

                    let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: _self.stored.clone() };

                    let (sx, sy) = (f64::from(in_data.downsample_x()), f64::from(in_data.downsample_y()));

                    let (w, h) = (params.get_f64(Params::OutputWidth).unwrap(), params.get_f64(Params::OutputHeight).unwrap());
                    let (x, y) = ((in_result.ref_width as f64 - w) / 2.0, (in_result.ref_height as f64 - h) / 2.0);

                    extra.set_result_rect(ae::Rect { left: (x * sx).round() as _, top: (y * sy).round() as _, right: ((x + w) * sx).round() as _, bottom: ((y + h) * sy).round() as _ });
                    extra.set_max_result_rect(extra.result_rect());
                    extra.set_returns_extra_pixels(true);

                    let full_rect = ae::Rect { left: 0, top: 0, right: w as _, bottom: h as _ };

                    if let Some(stab) = _self.stab_manager(&mut params, plugin.global, full_rect) {
                        {
                            let duration_ms = stab.params.read().duration_ms;
                            let old_range = stab.trim_ranges().first().cloned().unwrap_or((0.0, 1.0));
                            let old_range_ms = ((old_range.0 * duration_ms).round() as i64, (old_range.1 * duration_ms).round() as i64);
                            let new_range = trim_range.unwrap_or((0.0f64, duration_ms / 1000.0));
                            let new_range_ms = ((new_range.0 * 1000.0).round() as i64, (new_range.1 * 1000.0).round() as i64);
                            if old_range_ms != new_range_ms {
                                log::info!("Trim range changed: {old_range_ms:?} != {new_range_ms:?}");

                                stab.set_trim_ranges(vec![((new_range.0 * 1000.0) / duration_ms, (new_range.1 * 1000.0) / duration_ms)]);
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
