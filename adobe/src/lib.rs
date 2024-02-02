
use after_effects as ae;
use after_effects_sys as ae_sys;

use gyroflow_plugin_base::*;
use gyroflow_plugin_base::gyroflow_core::GyroflowCoreError;
use lru::LruCache;
use parking_lot::RwLock;
use std::sync::Arc;

mod parameters;
use parameters::*;

use serde::{ Serialize, Deserialize };

static mut IS_PREMIERE: bool = false;

#[derive(Default)]
struct Plugin {
    gyroflow: GyroflowPluginBase
}

#[derive(Serialize, Deserialize)]
struct Instance {
    #[serde(serialize_with="ser_stored", deserialize_with="de_stored")]
    gyroflow: GyroflowPluginBaseInstance<ParamHandler>
}
ae::define_cross_thread_type!(Instance);

impl Instance {
    fn new_base_instance() -> GyroflowPluginBaseInstance<ParamHandler>{
        log::info!("new_base_instance");
        let gyroflow = GyroflowPluginBaseInstance {
            parameters:                     ParamHandler::default(),
            managers:                       LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
            original_output_size:           (0, 0),
            original_video_size:            (0, 0),
            num_frames:                     0,
            fps:                            0.0,
            reload_values_from_project:     false,
            ever_changed:                   false,
            opencl_disabled:                false,
            cache_keyframes_every_frame:    true,
            framebuffer_inverted:           unsafe { IS_PREMIERE },
            keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                use_gyroflows_keyframes:  false, // TODO param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                cached_keyframes:         KeyframeManager::default()
            })),
        };
        gyroflow
    }
}

impl Default for Instance {
    fn default() -> Self {
        log::info!("Instance::default");
        let mut gyroflow = Self::new_base_instance();
        gyroflow.initialize_instance_id();
        Self {
            gyroflow,
        }
    }
}

impl Instance {
    fn stab_manager(&mut self, global: &mut Plugin, bit_depth: usize, input_rect: ae::Rect, output_rect: ae::Rect) -> Option<Arc<StabilizationManager>> {
        let in_size = (input_rect.width() as usize, input_rect.height() as usize);
        let out_size = (output_rect.width() as usize, output_rect.height() as usize);

        log::info!("in_size: {in_size:?} -> out_size: {out_size:?}, bit_depth: {bit_depth}");

        self.gyroflow.stab_manager(&global.gyroflow.manager_cache, bit_depth, in_size, out_size, false).ok()
    }

    fn smart_render(plugin: &PluginState, extra: SmartRenderExtra, is_gpu: bool) -> Result<(), ae::Error> {
        let in_data = plugin.in_data;
        let cb = extra.callbacks();
        let stab = extra.pre_render_data::<Arc<StabilizationManager>>();
        if stab.is_none() {
            log::error!("empty stab data in smart_render");
            return Ok(());
        }
        let input_world = cb.checkout_layer_pixels(in_data.effect_ref(), 0)?;
        if let Ok(output_world) = cb.checkout_output(in_data.effect_ref()) {
            if let Ok(world_suite) = ae::WorldSuite2::new() {
                let pixel_format = world_suite.get_pixel_format(input_world).unwrap();
                if is_gpu && pixel_format != ae::PixelFormat::GpuBgra128 {
                    log::info!("GPU render requested but pixel format is not GpuBgra128. It's: {:?}", pixel_format);
                    return Err(Error::UnrecogizedParameterType);
                }
                if let Some(stab) = stab {
                    log::info!("pixel_format: {pixel_format:?}, is_gpu: {is_gpu}, arc count: {}", Arc::strong_count(&stab));
                    log::info!("smart_render: {}, size: {:?}", in_data.current_timestamp(), stab.params.read().size);
                    log::info!("smart_render: time: {}, time_step: {}, time_scale: {}, frame: {}, local_frame: {}", in_data.current_time(), in_data.time_step(), in_data.time_scale(), in_data.current_frame(), in_data.current_frame_local());

                    let timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

                    let org_ratio = {
                        let params = stab.params.read();
                        params.video_size.0 as f64 / params.video_size.1 as f64
                    };

                    let src_size = (input_world.width(), input_world.height(), input_world.row_bytes());
                    let dest_size = (output_world.width(), output_world.height(), output_world.row_bytes());
                    let src_rect = GyroflowPluginBase::get_center_rect(input_world.width(),  input_world.height(), org_ratio);

                    let what_gpu = extra.what_gpu();
                    log::info!("Render API: {what_gpu:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dest_size: {dest_size:?}");

                    let gpu_suite = ae::pf::GPUDeviceSuite1::new();

                    let buffers = if is_gpu && !gpu_suite.is_err() {
                        let gpu_suite = gpu_suite.unwrap(); // Safe because we checked for error above
                        let device_info = gpu_suite.get_device_info(in_data, extra.device_index())?;

                        let in_ptr = gpu_suite.get_gpu_world_data(in_data, input_world)?;
                        let out_ptr = gpu_suite.get_gpu_world_data(in_data, output_world)?;

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
                            BufferSource::Cpu { buffer: unsafe { std::slice::from_raw_parts_mut(input_world.data_as_ptr_mut(),  src_size.1  * src_size.2) } },
                            BufferSource::Cpu { buffer: unsafe { std::slice::from_raw_parts_mut(output_world.data_as_ptr_mut(), dest_size.1 * dest_size.2) } },
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
                        ae::PixelFormat::Argb128    => stab.process_pixels::<RGBAf>(timestamp_us, &mut buffers),
                        ae::PixelFormat::Argb64     => stab.process_pixels::<RGBA16>(timestamp_us, &mut buffers),
                        ae::PixelFormat::Argb32     => stab.process_pixels::<RGBA8>(timestamp_us, &mut buffers),
                        _ => Err(GyroflowCoreError::UnsupportedFormat(format!("{pixel_format:?}")))
                    };
                    match result {
                        Ok(i)  => { log::info!("process_pixels ok: {i:?}"); },
                        Err(e) => { log::error!("process_pixels error: {e:?}"); }
                    }
                }
            }
        }
        cb.checkin_layer_pixels(in_data.effect_ref(), 0).unwrap();

        Ok(())
    }

    fn cpu_render(in_data: ae::InData, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        log::info!("render: {}", in_data.current_timestamp());

        if let Some(stab) = in_data.frame_data::<Arc<StabilizationManager>>() {
            let timestamp_us = (in_data.current_timestamp() * 1_000_000.0).round() as i64;

            let org_ratio = {
                let params = stab.params.read();
                params.video_size.0 as f64 / params.video_size.1 as f64
            };

            let src_size = (src.width() as usize, src.height() as usize, src.buffer_stride());
            let dst_size = (dst.width() as usize, dst.height() as usize, dst.buffer_stride());
            let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);

            log::info!("org_ratio: {org_ratio:?}, src_size: {src_size:?}, src_rect: {src_rect:?}, dst_size: {dst_size:?}, src.stride: {}, bit_depth: {}", src.rowbytes(), src.bit_depth());

            let src_buffer = unsafe { std::slice::from_raw_parts_mut(src.buffer().as_ptr() as *mut u8, src.buffer().len()) };
            let dst_buffer = unsafe { std::slice::from_raw_parts_mut(dst.buffer().as_ptr() as *mut u8, dst.buffer().len()) };

            let mut buffers = Buffers {
                input:  BufferDescription { size: src_size, rect: Some(src_rect), data: BufferSource::Cpu { buffer: src_buffer }, rotation: None, texture_copy: false },
                output: BufferDescription { size: dst_size, rect: None,           data: BufferSource::Cpu { buffer: dst_buffer }, rotation: None, texture_copy: false }
            };
            let result = match src.bit_depth() {
                8  => stab.process_pixels::<RGBA8> (timestamp_us, &mut buffers),
                16 => stab.process_pixels::<RGBA16>(timestamp_us, &mut buffers),
                32 => stab.process_pixels::<RGBAf> (timestamp_us, &mut buffers),
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

    fn params_setup(&self, params: &mut ae::Parameters<Params>) -> Result<(), Error> {
        for param in GyroflowPluginBase::get_param_definitions() {
            define_param(params, param, None);
        }
        Ok(())
    }

    fn handle_command(&mut self, cmd: ae::Command, in_data: ae::InData, mut out_data: ae::OutData) -> Result<(), ae::Error> {
        let _ = log::set_logger(&win_dbg_logger::DEBUGGER_LOGGER);
        log::set_max_level(log::LevelFilter::Debug);
        log_panics::init();

        log::info!("handle_command: {:?}, thread: {:?}, ptr: {:?}, effect_ref: {:?}", cmd, std::thread::current().id(), self as *const _, in_data.effect_ref().as_ptr());

        match cmd {
            ae::Command::About => {
                out_data.set_return_msg("Gyroflow, v0.1\nCopyright 2024 AdrianEddy\rGyroflow plugin.");
            }
            ae::Command::GlobalSetup => {
                self.gyroflow.initialize_gpu_context();

                if &in_data.application_id() == b"PrMr" {
                    unsafe { IS_PREMIERE = true; }
                    use ae::pr::PixelFormat::*;
                    let pixel_format = ae::pf::PixelFormatSuite::new()?;
                    pixel_format.clear_supported_pixel_formats(in_data.effect_ref())?;
                    let supported_formats = [
                        Bgra4444_8u,  Vuya4444_8u,  Vuya4444_8u709,  Argb4444_8u,  Bgrx4444_8u,  Vuyx4444_8u,  Vuyx4444_8u709,  Xrgb4444_8u,  Bgrp4444_8u,  Vuyp4444_8u,  Vuyp4444_8u709,  Prgb4444_8u,
                        Bgra4444_16u, Vuya4444_16u,                  Argb4444_16u, Bgrx4444_16u,                                Xrgb4444_16u, Bgrp4444_16u,                                Prgb4444_16u,
                        Bgra4444_32f, Vuya4444_32f, Vuya4444_32f709, Argb4444_32f, Bgrx4444_32f, Vuyx4444_32f, Vuyx4444_32f709, Xrgb4444_32f, Bgrp4444_32f, Vuyp4444_32f, Vuyp4444_32f709, Prgb4444_32f,
                        Bgra4444_32fLinear, Bgrp4444_32fLinear, Bgrx4444_32fLinear, Argb4444_32fLinear, Prgb4444_32fLinear, Xrgb4444_32fLinear
                    ];
                    for x in supported_formats {
                        pixel_format.add_pr_supported_pixel_format(in_data.effect_ref(), x)?;
                    }
                    out_data.set_out_flag2(ae_sys::PF_OutFlag2_SUPPORTS_GPU_RENDER_F32, false);
                } else {
                    out_data.set_out_flag2(ae_sys::PF_OutFlag2_SUPPORTS_GPU_RENDER_F32, true);
                }
            }
            ae::Command::GlobalSetdown => {
                //self.manager_cache.lock().clear();
            }
            ae::Command::GpuDeviceSetup { extra } => {
                let device_info = ae::pf::GPUDeviceSuite1::new().unwrap().get_device_info(in_data, extra.device_index())?;

                let what_gpu = extra.what_gpu();

                log::info!("Device info: {device_info:?}. GPU: {what_gpu:?}");

                if what_gpu != ae::GpuFramework::None {
                    out_data.set_out_flag2(ae_sys::PF_OutFlag2_SUPPORTS_GPU_RENDER_F32, true);
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

impl AdobePluginInstance for CrossThreadInstance {
    fn flatten(&self) -> Result<(u16, Vec<u8>), Error> {
        let bytes = bincode::serialize(&self).unwrap();
        log::info!("flatten, bytes: {}", pretty_hex::pretty_hex(&bytes));
        Ok((1, bytes))
    }
    fn unflatten(version: u16, bytes: &[u8]) -> Result<Self, Error> {
        log::info!("unflatten version: {version} bytes: {}", pretty_hex::pretty_hex(&bytes));
        let inst = bincode::deserialize::<Self>(bytes).unwrap_or_default();
        Ok(inst)
    }

    fn user_changed_param(&mut self, plugin: &mut PluginState, param: Params) -> Result<(), ae::Error> {
        let _self = self.get().unwrap();
        let mut _self = _self.write();
        _self.gyroflow.parameters.fields.params = plugin.params.clone();
        if let Err(e) = _self.gyroflow.param_changed(&plugin.global.gyroflow.manager_cache, param, true) {
            log::error!("param_changed error: {e:?}");
        }
        Ok(())
    }
    fn do_dialog(&mut self, plugin: &mut PluginState) -> Result<(), ae::Error> {
        self.user_changed_param(plugin, Params::OpenGyroflow)
    }

    fn render(&self, plugin: &mut PluginState, src: &Layer, dst: &mut Layer) -> Result<(), ae::Error> {
        Instance::cpu_render(plugin.in_data, src, dst)
    }

    fn handle_command(&mut self, plugin: &mut PluginState, cmd: ae::Command) -> Result<(), ae::Error> {
        log::info!("sequence command: {:?}, thread: {:?}, ptr: {:?}", cmd, std::thread::current().id(), self as *const _);

        let in_data = &mut plugin.in_data;

        match cmd {
            ae::Command::UserChangedParam { .. } => {
                plugin.out_data.set_force_rerender();
            }
            ae::Command::UpdateParamsUi => {
                /*let _self = self.get().unwrap();
                let mut _self = _self.write();
                _self.gyroflow.parameters.fields.params = plugin.params.clone();
                let path = _self.gyroflow.parameters.get_string(Params::ProjectPath).map(|x| !x.is_empty());
                _self.gyroflow.update_loaded_state(path.unwrap_or_default());*/
            }
            ae::Command::SequenceSetup => {
                if let Ok(interface_suite) = ae::aegp::PFInterfaceSuite::new() {
                    if let Ok(layer_suite) = ae::aegp::LayerSuite::new() {
                        if let Ok(footage_suite) = ae::aegp::FootageSuite::new() {
                            let layer = interface_suite.effect_layer(in_data.effect_ref())?;
                            let item = layer_suite.layer_source_item(layer)?;
                            let footage = footage_suite.main_footage_from_item(item)?;
                            let footage_path = footage_suite.footage_path(footage, 0, 0);
                            log::info!("footage path: {:?}", footage_path);
                            if let Ok(footage_path) = footage_path {
                                if !footage_path.is_empty() {
                                    let project_path = GyroflowPluginBase::get_project_path(&footage_path).unwrap_or(footage_path.to_owned());
                                    let _self = self.get().unwrap();
                                    _self.write().gyroflow.parameters.fields.stored.project_path = project_path;
                                }
                            }
                        }
                    }
                }
            },
            ae::Command::SequenceSetdown => {
                let _self = self.get().unwrap();
                _self.write().gyroflow.clear_stab(&plugin.global.gyroflow.manager_cache);
            },
            ae::Command::SmartPreRender { mut extra } => {
                let what_gpu = extra.what_gpu();
                let req = extra.output_request();

                if what_gpu != ae::GpuFramework::None {
                    extra.set_gpu_render_possible(true);
                }

                let cb = extra.callbacks();
                if let Ok(in_result) = cb.checkout_layer(in_data.effect_ref(), 0, 0, &req, in_data.current_time(), in_data.time_step(), in_data.time_scale()) {
                    let      result_rect = extra.union_result_rect(in_result.result_rect.into());
                    let _max_result_rect = extra.union_max_result_rect(in_result.max_result_rect.into());

                    let _self = self.get().unwrap();
                    let mut _self = _self.write();
                    _self.gyroflow.parameters.fields.params = plugin.params.clone();

                    if let Some(stab) = _self.stab_manager(plugin.global, extra.bit_depth() as usize, result_rect, result_rect) {
                        log::info!("setting pre-render extra: {result_rect:?}, in: {:?}", in_data.extent_hint());
                        extra.set_pre_render_data::<Arc<StabilizationManager>>(stab);
                    } else {
                        return Err(ae::Error::Generic);
                    }
                }
            }
            ae::Command::FrameSetup { out_layer, .. } => {
                let _self = self.get().unwrap();
                let mut _self = _self.write();
                _self.gyroflow.parameters.fields.params = plugin.params.clone();

                if let Some(stab) = _self.stab_manager(plugin.global, out_layer.bit_depth() as usize, in_data.extent_hint(), out_layer.extent_hint()) {
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
            ae::Command::Event { extra } => {
                log::info!("Event: {extra:?}");
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

ae::register_plugin!(Plugin, CrossThreadInstance, Params);
