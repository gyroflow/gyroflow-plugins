
use after_effects as ae;
use premiere as pr;


use gyroflow_plugin_base::*;
use gyroflow_plugin_base::gyroflow_core::GyroflowCoreError;
use lru::LruCache;
use parking_lot::RwLock;
use std::sync::Arc;
use ae::aegp::{ LayerFlags, LayerStream, TimeMode, PluginId, StreamValue };

mod ui;

mod parameters;
use parameters::*;

use serde::{ Serialize, Deserialize };

static mut AEGP_PLUGIN_ID: PluginId = 0;
static mut IS_PREMIERE: bool = false;
static GLOBAL_INST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
fn global_inst<'a>() -> &'a mut Plugin {
    unsafe {
        let ptr = (*GLOBAL_INST.get_or_init(|| 0)) as *mut Plugin;
        &mut *ptr
    }
}

#[derive(Default)]
struct Plugin {
    gyroflow: GyroflowPluginBase
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct StoredParams {
    pub version: u8,
    pub media_file_path: String,
    pub project_path: String,
    pub instance_id: String,
    pub status: String,
    pub sequence_size: (usize, usize),
    pub media_fps_ticks: i64,
}
impl Default for StoredParams {
    fn default() -> Self {
        Self {
            version: 1,
            media_file_path: String::new(),
            project_path: String::new(),
            instance_id: format!("{}", fastrand::u64(..)),
            status: "Project not loaded".to_owned(),
            sequence_size: (0, 0),
            media_fps_ticks: 0,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Instance {
    #[serde(serialize_with="ser_stored", deserialize_with="de_stored")]
    stored: Arc<RwLock<StoredParams>>,
    #[serde(skip)]
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
            reload_values_from_project:     false,
            ever_changed:                   false,
            opencl_disabled:                false,
            cache_keyframes_every_frame:    true,
            framebuffer_inverted:           false, //unsafe { IS_PREMIERE },
            keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                use_gyroflows_keyframes:  false, // TODO param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                cached_keyframes:         KeyframeManager::default()
            })),
        };
        gyroflow.initialize_instance_id(instance_id);
        gyroflow
    }
}

impl Default for Instance {
    fn default() -> Self {
        log::info!("Instance::default");
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
        let stab = extra.pre_render_data::<Arc<StabilizationManager>>();
        if stab.is_none() {
            log::error!("empty stab data in smart_render");
            return Ok(());
        }
        let mut input_world = cb.checkout_layer_pixels(0)?;
        if let Ok(mut output_world) = cb.checkout_output() {
            if let Ok(world_suite) = ae::pf::suites::World::new() {
                let pixel_format = world_suite.pixel_format(&input_world).unwrap();
                if is_gpu && pixel_format != ae::PixelFormat::GpuBgra128 {
                    log::info!("GPU render requested but pixel format is not GpuBgra128. It's: {:?}", pixel_format);
                    return Err(Error::UnrecogizedParameterType);
                }
                if let Some(stab) = stab {
                    //log::info!("pixel_format: {pixel_format:?}, is_gpu: {is_gpu}, arc count: {}", Arc::strong_count(&stab));
                    //log::info!("smart_render: {}, size: {:?}", in_data.current_timestamp(), stab.params.read().size);
                    log::info!("smart_render: timestamp: {} time: {}, time_step: {}, time_scale: {}, frame: {}, local_frame: {}",
                        in_data.current_timestamp(),
                        in_data.current_time(),
                        in_data.time_step(),
                        in_data.time_scale(),
                        in_data.current_frame(),
                        in_data.current_frame_local()
                    );

                    let mut timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;
                    log::info!("timestamp_us: {timestamp_us}");

                    let (org_ratio, fps) = {
                        let params = stab.params.read();
                        (params.size.0 as f64 / params.size.1 as f64, params.fps)
                    };

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
                        let frame = timestamp_us as f64 * (fps / 1_000_000.0);
                        log::info!("frame: {frame}");
                        let frame = if frame.fract() > 0.999 { frame.ceil() } else { frame.floor() };
                        timestamp_us = (frame.floor() * (1_000_000.0 / fps)).round() as i64;
                        log::info!("timestamp_us: {timestamp_us}");
                    }

                    let src_size = (input_world.width(), input_world.height(), input_world.buffer_stride());
                    let dest_size = (output_world.width(), output_world.height(), output_world.buffer_stride());
                    let src_rect = GyroflowPluginBase::get_center_rect(input_world.width(),  input_world.height(), org_ratio);

                    let what_gpu = extra.what_gpu();
                    log::info!("Render API: {what_gpu:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dest_size: {dest_size:?}");

                    let gpu_suite = ae::pf::suites::GPUDevice::new();

                    let buffers = if is_gpu && !gpu_suite.is_err() {
                        let gpu_suite = gpu_suite.unwrap(); // Safe because we checked for error above
                        let device_info = gpu_suite.device_info(in_data.effect_ref(), extra.device_index())?;

                        let in_ptr = gpu_suite.gpu_world_data(in_data.effect_ref(), input_world)?;
                        let out_ptr = gpu_suite.gpu_world_data(in_data.effect_ref(), output_world)?;

                        log::info!("Render GPU: {in_ptr:?} -> {out_ptr:?}. API: {what_gpu:?}, pixel_format: {pixel_format:?}");

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
                                BufferSource::MetalBuffer { buffer: out_ptr as *mut metal::MTLBuffer, command_queue: device_info.command_queuePV as *mut metal::MTLCommandQueue },
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
                    let mut buffers = Buffers {
                        input:  BufferDescription { size: src_size,  rect: Some(src_rect), data: buffers.0, rotation: None, texture_copy: buffers.2 },
                        output: BufferDescription { size: dest_size, rect: None,           data: buffers.1, rotation: None, texture_copy: buffers.2 }
                    };
                    log::info!("pixel_format: {pixel_format:?}");
                    let result = match pixel_format {
                        ae::PixelFormat::GpuBgra128 |
                        ae::PixelFormat::Argb128    => stab.process_pixels::<RGBAf>(timestamp_us, None, &mut buffers),
                        ae::PixelFormat::Argb64     => stab.process_pixels::<RGBA16>(timestamp_us, None, &mut buffers),
                        ae::PixelFormat::Argb32     => stab.process_pixels::<RGBA8>(timestamp_us, None, &mut buffers),
                        _ => Err(GyroflowCoreError::UnsupportedFormat(format!("{pixel_format:?}")))
                    };
                    match result {
                        Ok(i)  => { log::info!("process_pixels ok: {i:?}"); },
                        Err(e) => { log::error!("process_pixels error: {e:?}"); }
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
        log::info!("render: {}", in_data.current_timestamp());

        if let Some(stab) = in_data.frame_data::<Arc<StabilizationManager>>() {
            let timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

            let org_ratio = {
                let params = stab.params.read();
                params.size.0 as f64 / params.size.1 as f64
            };

            let src_size = (src.width() as usize, src.height() as usize, src.buffer_stride());
            let dst_size = (dst.width() as usize, dst.height() as usize, dst.buffer_stride());
            let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);

            log::info!("org_ratio: {org_ratio:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dst_size: {dst_size:?}, src.stride: {}, bit_depth: {}", src.row_bytes(), src.bit_depth());

            let src_buffer = unsafe { std::slice::from_raw_parts_mut(src.buffer().as_ptr() as *mut u8, src.buffer().len()) };
            let dst_buffer = unsafe { std::slice::from_raw_parts_mut(dst.buffer().as_ptr() as *mut u8, dst.buffer().len()) };

            let mut buffers = Buffers {
                input:  BufferDescription { size: src_size, rect: Some(src_rect), data: BufferSource::Cpu { buffer: src_buffer }, rotation: None, texture_copy: false },
                output: BufferDescription { size: dst_size, rect: None,           data: BufferSource::Cpu { buffer: dst_buffer }, rotation: None, texture_copy: false }
            };
            let result = match src.bit_depth() {
                8  => stab.process_pixels::<RGBA8> (timestamp_us, None, &mut buffers),
                16 => stab.process_pixels::<RGBA16>(timestamp_us, None, &mut buffers),
                32 => stab.process_pixels::<RGBAf> (timestamp_us, None, &mut buffers),
                bd => panic!("Unknown bit depth: {bd}")
            };
            match result {
                Ok(i)  => { log::info!("process_pixels ok: {i:?}"); },
                Err(e) => { log::error!("process_pixels error: {e:?}"); }
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

    fn handle_command(&mut self, cmd: ae::Command, in_data: InData, mut out_data: OutData, params: &mut ae::Parameters<Params>) -> Result<(), ae::Error> {
        self.gyroflow.initialize_log();

        // log::info!("global command: {:?}, thread: {:?}, ptr: {:?}, effect_ref: {:?}", cmd, std::thread::current().id(), self as *const _, in_data.effect_ref().as_ptr());

        match cmd {
            ae::Command::About => {
                out_data.set_return_msg("Gyroflow, v0.1\nCopyright 2024 AdrianEddy\rGyroflow plugin.");
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
                let device_info = ae::pf::suites::GPUDevice::new().unwrap().device_info(in_data.effect_ref(), extra.device_index())?;

                let what_gpu = extra.what_gpu();

                log::info!("Device info: {device_info:?}. GPU: {what_gpu:?}");

                if what_gpu != ae::GpuFramework::None {
                    out_data.set_out_flag2(ae::OutFlags2::SupportsGpuRenderF32, true);
                }
            }
            ae::Command::GpuDeviceSetdown { extra } => {
                log::info!("gpu_device_setdown: {:?}", extra.what_gpu());
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
        let _self = self.get().unwrap();

        let mut _self = _self.write();
        let stored = _self.stored.clone();
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
            //let current_instance_id = params.get_string(Params::InstanceId).unwrap_or_default();
            if let Err(e) = inst.param_changed(&mut params, &plugin.global.gyroflow.manager_cache, param, true) {
                log::error!("param_changed error: {e:?}");
            }

            /*if current_instance_id != params.get_string(Params::InstanceId).unwrap_or_default() {
                log::warn!("Instance ID changed, creating new cross thread instance!");
                self.id = fastrand::u64(..);
            }*/

            let _ = inst.stab_manager(&mut params, &plugin.global.gyroflow.manager_cache, (0, 0), false);
        }

        Ok(())
    }
}

impl AdobePluginInstance for CrossThreadInstance {
    fn flatten(&self) -> Result<(u16, Vec<u8>), Error> {
        let bytes = bincode::serialize(&self).unwrap();
        log::info!("flatten, len: {}", bytes.len());
        log::info!("bytes: {}", pretty_hex::pretty_hex(&bytes));
        Ok((1, bytes))
    }
    fn unflatten(version: u16, bytes: &[u8]) -> Result<Self, Error> {
        log::info!("unflatten version: {version} bytes: {}", pretty_hex::pretty_hex(&bytes));
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

        // let _pica = pr::PicaBasicSuite::from_sp_basic_suite_raw(in_data.pica_basic_suite_ptr() as *const _);

        /*if !matches!(cmd, ae::Command::GlobalSetup | ae::Command::ParamsSetup) {
            plugin.global.set_global_ptr(&in_data);
        }*/

        match cmd {
            ae::Command::UserChangedParam { param_index } => {
                self.user_changed_param(plugin, plugin.params.type_at(param_index))?;

                plugin.out_data.set_out_flag(ae::OutFlags::RefreshUi, true);
                plugin.out_data.set_force_rerender();
            }
            ae::Command::UpdateParamsUi => {
                //let _self = self.get().unwrap();
                //let mut _self = _self.write();
                //let mut params = ParamHandler { inner: plugin.params, stored: _self.stored.clone() };
                //let _ = _self.stab_manager(&mut params, plugin.global, 8, ae::Rect::empty(), ae::Rect::empty());

                //let _self = self.get().unwrap();
                //let mut _self = _self.write();
                //if !_self.stored.read().project_path.is_empty() {
                //    log::info!("project path2: {}", _self.stored.read().project_path);
                //    let mut params = ParamHandler { inner: plugin.params, stored: _self.stored.clone() };
                //    params.set_string(Params::ProjectPath, &_self.stored.read().project_path).unwrap();
                //    _self.stored.write().project_path.clear();
                //}

                //let mut params = ParamHandler { inner: plugin.params };

                //let has_motion = _self.gyroflow.has_motion;
                //_self.gyroflow.update_loaded_state(&mut params, has_motion);
                //plugin.out_data.set_out_flag(ae::OutFlags::RefreshUi, true);
                //plugin.out_data.set_force_rerender();
            }
            ae::Command::SequenceSetup => {
                let _self = self.get().unwrap();
                let _self = _self.read();
                let mut stored = _self.stored.write();
                stored.sequence_size = (in_data.width() as _, in_data.height() as _);

                let _ = (|| -> Result<(), ae::Error> {
                    let layer = in_data.effect().layer()?;
                    let footage_path = layer
                                      .source_item()?
                                      .main_footage()?
                                      .path(0, 0)?;
                    if !footage_path.is_empty() {
                        stored.project_path = GyroflowPluginBase::get_project_path(&footage_path).unwrap_or(footage_path.to_owned());
                        stored.media_file_path = footage_path;
                    }

                    let comp_dimensions = layer
                                         .parent_comp()?
                                         .item()?
                                         .dimensions()?;
                    stored.sequence_size = (comp_dimensions.0 as _, comp_dimensions.1 as _);
                    Ok(())
                })();
            }
            ae::Command::SequenceSetdown => {
                //let _self = self.get().unwrap();
                //_self.write().gyroflow.as_mut().unwrap().clear_stab(&plugin.global.gyroflow.manager_cache);
            },
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

                    let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: _self.stored.clone() };

                    let (sx, sy) = (f64::from(in_data.downsample_x()), f64::from(in_data.downsample_y()));

                    let (w, h) = ((params.get_f64(Params::OutputWidth).unwrap() * sx).round() as i32, (params.get_f64(Params::OutputHeight).unwrap() * sy).round() as i32);
                    let (x, y) = ((in_result.ref_width - w) / 2, (in_result.ref_height - h) / 2);

                    extra.set_result_rect(ae::Rect { left: x, top: y, right: x + w, bottom: y + h });
                    extra.set_max_result_rect(extra.result_rect());
                    extra.set_returns_extra_pixels(true);

                    if let Some(stab) = _self.stab_manager(&mut params, plugin.global, extra.result_rect()) {
                        log::info!("setting pre-render extra: {:?}, in: {:?}", extra.result_rect(), in_data.extent_hint());
                        extra.set_pre_render_data::<Arc<StabilizationManager>>(stab);
                    } else {
                        return Err(ae::Error::Generic);
                    }
                }
            }
            ae::Command::FrameSetup { out_layer, .. } => {
                let _self = self.get().unwrap();
                let mut _self = _self.write();

                let mut params = ParamHandler { inner: ParamsInner::Ae(plugin.params), stored: _self.stored.clone() };

                // Output buffer resizing may only occur during FrameSetup.
                let (sx, sy) = (f64::from(in_data.downsample_x()), f64::from(in_data.downsample_y()));
                let (nw, nh) = ((params.get_f64(Params::OutputWidth).unwrap() * sx).round() as u32, (params.get_f64(Params::OutputHeight).unwrap() * sy).round() as u32);
                plugin.out_data.set_width(nw as _);
                plugin.out_data.set_height(nh as _);

                if let Some(stab) = _self.stab_manager(&mut params, plugin.global, out_layer.extent_hint()) {
                    plugin.out_data.set_frame_data::<Arc<StabilizationManager>>(stab);
                } else {
                    log::error!("frame_setup: no stab manager");
                }
            }
            ae::Command::FrameSetdown => {
                in_data.destroy_frame_data::<Arc<StabilizationManager>>();
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

struct PremiereGPU;
impl Default for PremiereGPU {
    fn default() -> Self { log::info!("creating PremiereGPU"); Self { }}
}
impl Drop for PremiereGPU {
    fn drop(&mut self) { log::info!("dropping PremiereGPU: {:?}", self as *const _); }
}

impl pr::GpuFilter for PremiereGPU {
    fn global_init() { }
    fn global_destroy() { }

    fn get_frame_dependencies(&self, _filter: &pr::GpuFilterData, _render_params: pr::RenderParams, _query_index: &mut i32) -> Result<pr::sys::PrGPUFilterFrameDependency, pr::Error> {
        Err(pr::Error::NotImplemented)
    }
    fn precompute(&self, _filter: &pr::GpuFilterData, _render_params: pr::RenderParams, _index: i32, _frame: pr::sys::PPixHand) -> Result<(), pr::Error> {
        Err(pr::Error::NotImplemented)
    }
    fn render(&self, filter: &pr::GpuFilterData, render_params: pr::RenderParams, frames: *const pr::sys::PPixHand, _frame_count: usize, out_frame: *mut pr::sys::PPixHand) -> Result<(), pr::Error> {
        let (frames, out_frame) = unsafe {
            (*filter.instance_ptr).outIsRealtime = 1;
            (*frames, *out_frame)
        };
        let pixel_format = filter.ppix_suite.pixel_format(out_frame).unwrap();

        let in_frame_data = filter.gpu_device_suite.gpu_ppix_data(frames).unwrap();
        let out_frame_data = filter.gpu_device_suite.gpu_ppix_data(out_frame).unwrap();

        let in_stride = filter.ppix_suite.row_bytes(frames).unwrap();
        let out_stride = filter.ppix_suite.row_bytes(out_frame).unwrap();

        let in_bounds  = filter.ppix_suite.bounds(frames).unwrap();
        let out_bounds = filter.ppix_suite.bounds(out_frame).unwrap();
        let in_size  = ( in_bounds.right -  in_bounds.left,  in_bounds.bottom -  in_bounds.top);
        let out_size = (out_bounds.right - out_bounds.left, out_bounds.bottom - out_bounds.top);

        if let Ok(pr::PropertyData::Binary(bytes)) = filter.property(pr::Property::Effect_FilterOpaqueData) {
            if bytes.len() > 2 {
                let inst = CrossThreadInstance::unflatten(1, &bytes[2..]).unwrap_or_default();

                let inst = inst.get().unwrap();
                let mut inst = inst.write();

                let clip_node = filter.video_segment_suite.acquire_operator_owner_node_id(filter.node_id())?;
                let media_node = filter.video_segment_suite.acquire_input_node_id(clip_node, 0)?;

                if inst.stored.read().media_file_path.is_empty() {
                    if let Ok(pr::PropertyData::String(media_path)) = filter.video_segment_suite.node_property(media_node.1, pr::Property::Media_InstanceString) {
                        let mut stored = inst.stored.write();
                        stored.project_path = GyroflowPluginBase::get_project_path(&media_path).unwrap_or(media_path.to_owned());
                        stored.media_file_path = media_path;
                        stored.sequence_size = (render_params.render_width() as _, render_params.render_height() as _);
                    }
                    if let Ok(pr::PropertyData::Time(media_fps)) = filter.video_segment_suite.node_property(media_node.1, pr::Property::Media_StreamFrameRate) {
                        inst.stored.write().media_fps_ticks = media_fps;
                    }
                    /*filter.video_segment_suite.iterate_node_properties(clip_node, |k, v| {
                        log::info!("clip_node Property {k:?} = {v:?}");
                    })?;
                    filter.video_segment_suite.iterate_node_properties(filter.node_id(), |k, v| {
                        log::info!("operator Property {k:?} = {v:?}");
                    })?;
                    filter.video_segment_suite.iterate_node_properties(media_node.1, |k, v| {
                        log::info!("Property {k:?} = {v:?}");
                    })?;*/
                    // Media_ClipSpeed
                    // Media_StreamPixelAspectRatioNum
                    // Media_StreamPixelAspectRatioDen
                    // Media_SequenceFrameRate
                    // Media_StreamFrameRate
                }
                if let Ok(pr::PropertyData::Keyframes(kf)) = filter.video_segment_suite.node_property(clip_node, pr::Property::Clip_TimeRemapping) {
                    log::info!("kf: {}", kf.0.replace("\\n", "\n"));
                }

                let mut params = ParamHandler { inner: ParamsInner::Premiere((filter, render_params.clone())), stored: inst.stored.clone() };

                let path = params.get_string(Params::ProjectPath).unwrap();
                let instance_id = params.get_string(Params::InstanceId).unwrap();
                let disable_stretch = params.get_bool(Params::DisableStretch).unwrap();

                let out_w = params.get_f64(Params::OutputWidth).unwrap();
                let out_h = params.get_f64(Params::OutputHeight).unwrap();

                let key = format!("{path}{disable_stretch}{instance_id}");
                //log::info!("key {key}, out_size {out_w}x{out_h}");

                //log::info!("PremiereGPU::render! {pixel_format:?} in: {in_frame_data:?}, out: {out_frame_data:?}, stride: {in_stride}/{out_stride}, bounds: {in_bounds:?}/{out_bounds:?}, disable_stretch: {disable_stretch:?} path: {} instance_id: {instance_id:?} | time: {}", path, render_params.clip_time());

                //log::info!("{:?}", inst.stored);

                let base_inst = inst.gyroflow.as_mut().unwrap();
                base_inst.timeline_size = (render_params.render_width() as _, render_params.render_height() as _);

                if let Ok(stab) = base_inst.stab_manager(&mut params, &global_inst().gyroflow.manager_cache, (out_size.0 as _, out_size.1 as _), false) {
                    static TICKS_PER_SEC: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
                    let ticks_per_sec = *TICKS_PER_SEC.get_or_init(|| pr::suites::Time::new().and_then(|x| x.ticks_per_second()).unwrap_or(254016000000) as f64);

                    let (org_ratio, fps) = {
                        let params = stab.params.read();
                        (params.size.0 as f64 / params.size.1 as f64, params.fps)
                    };

                    let fps_ticks = inst.stored.read().media_fps_ticks;
                    let fps_ticks = if fps_ticks == 0 { ticks_per_sec as f64 / fps } else { fps_ticks as f64 };

                    // round the timestamp_us according to the fps, so it's never between frames and always points to a valid frame timestamp
                    let frame = render_params.clip_time() as f64 / fps_ticks;
                    log::info!("frame: {frame}");
                    let frame = if frame.fract() > 0.999 { frame.ceil() } else { frame.floor() };
                    let timestamp_us = (frame * (1_000_000.0 / fps)).round() as i64;
                    log::info!("timestamp_us2: {timestamp_us}");

                    let src_size = (in_size.0 as usize, in_size.1 as usize, in_stride as usize);
                    let dest_size = (out_size.0 as usize, out_size.1 as usize, out_stride as usize);
                    let src_rect = GyroflowPluginBase::get_center_rect(in_size.0 as usize, in_size.1 as usize, org_ratio);
                    let out_rect = GyroflowPluginBase::get_center_rect(out_size.0 as usize, out_size.1 as usize, out_w as f64 / out_h.max(1.0) as f64);

                    let in_ptr = in_frame_data;
                    let out_ptr = out_frame_data;

                    let api = filter.gpu_info.outDeviceFramework;

                    //log::info!("Render GPU: {in_ptr:?} -> {out_ptr:?}. API: {:?}, pixel_format: {pixel_format:?} {src_rect:?}->{out_rect:?}", api);

                    let buffers = match api {
                        #[cfg(any(target_os = "windows", target_os = "linux"))]
                        pr::sys::PrGPUDeviceFramework_PrGPUDeviceFramework_CUDA => (
                            BufferSource::CUDABuffer { buffer: in_ptr },
                            BufferSource::CUDABuffer { buffer: out_ptr },
                            true
                        ),
                        #[cfg(any(target_os = "macos", target_os = "ios"))]
                        pr::sys::PrGPUDeviceFramework_PrGPUDeviceFramework_Metal => (
                            BufferSource::MetalBuffer { buffer: in_ptr  as *mut metal::MTLBuffer, command_queue: filter.gpu_info.outCommandQueueHandle as *mut metal::MTLCommandQueue },
                            BufferSource::MetalBuffer { buffer: out_ptr as *mut metal::MTLBuffer, command_queue: filter.gpu_info.outCommandQueueHandle as *mut metal::MTLCommandQueue },
                            true
                        ),
                        pr::sys::PrGPUDeviceFramework_PrGPUDeviceFramework_OpenCL => (
                            BufferSource::OpenCL { texture: in_ptr,  queue: filter.gpu_info.outCommandQueueHandle },
                            BufferSource::OpenCL { texture: out_ptr, queue: std::ptr::null_mut() },
                            true
                        ),
                        _ => panic!("Invalid GPU framework")
                    };

                    let mut buffers = Buffers {
                        input:  BufferDescription { size: src_size,  rect: Some(src_rect), data: buffers.0, rotation: None, texture_copy: buffers.2 },
                        output: BufferDescription { size: dest_size, rect: Some(out_rect), data: buffers.1, rotation: None, texture_copy: buffers.2 }
                    };
                    let result = match pixel_format {
                        pr::PixelFormat::GpuBgra4444_32f => stab.process_pixels::<RGBAf>(timestamp_us, None, &mut buffers),
                        pr::PixelFormat::GpuBgra4444_16f => stab.process_pixels::<RGBAf16>(timestamp_us, None, &mut buffers),
                        _ => Err(GyroflowCoreError::UnsupportedFormat(format!("{pixel_format:?}")))
                    };
                    match result {
                        Ok(i)  => { log::info!("process_pixels ok: {i:?}"); },
                        Err(e) => { log::error!("process_pixels error: {e:?}"); }
                    }
                } else {
                    log::info!("!!!!!!!!!! Key not found: {key}");
                }
            }
        }
        Ok(())
    }
}

ae::define_effect!(Plugin, CrossThreadInstance, Params);
pr::define_gpu_filter!(PremiereGPU);
