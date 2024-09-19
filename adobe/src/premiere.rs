
use premiere as pr;
use gyroflow_plugin_base::{ *, gyroflow_core::GyroflowCoreError };
use super::{ parameters::*, CrossThreadInstance, AdobePluginInstance };

#[derive(Default)]
struct PremiereGPU;

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

                {
                    let keyframe_test = [ Params::Fov, Params::Smoothness, Params::ZoomLimit, Params::LensCorrectionStrength,
                                          Params::HorizonLockAmount, Params::HorizonLockRoll, Params::VideoSpeed, Params::Rotation,
                                          Params::AdditionalYaw, Params::AdditionalPitch ];
                    let mut stored = inst.stored.write();
                    stored.premiere_keyframed_params.clear();
                    for kf in keyframe_test {
                        if let Some(ind) = param_index_for_type(kf, None) {
                            if filter.next_keyframe_time(ind, -1) != Err(pr::Error::NoKeyframeAfterInTime) {
                                stored.premiere_keyframed_params.insert(kf);
                            }
                        }
                    }
                }

                if inst.stored.read().media_file_path.is_empty() {
                    if let Ok(pr::PropertyData::String(media_path)) = filter.video_segment_suite.node_property(media_node.1, pr::Property::Media_InstanceString) {
                        let mut stored = inst.stored.write();
                        stored.pending_params_str.insert(Params::ProjectPath, GyroflowPluginBase::get_project_path(&media_path).unwrap_or(media_path.to_owned()));
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

                static TICKS_PER_SEC: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
                let ticks_per_sec = *TICKS_PER_SEC.get_or_init(|| pr::suites::Time::new().and_then(|x| x.ticks_per_second()).unwrap_or(254016000000) as f64);

                let mut params = ParamHandler { inner: ParamsInner::Premiere((filter, render_params.clone())), stored: inst.stored.clone() };

                if params.get_bool(Params::StabilizationSpeedRamp).unwrap_or_default() {
                    if let Ok(pr::PropertyData::Keyframes(_)) = filter.video_segment_suite.node_property(clip_node, pr::Property::Clip_TimeRemapping) {
                        if let Ok(pr::PropertyData::Time(media_fps)) = filter.video_segment_suite.node_property(media_node.1, pr::Property::Media_StreamFrameRate) {
                            let fps = ticks_per_sec / media_fps as f64;

                            let mut speed_per_frame = Vec::new();
                            let mut prev_ticks = 0;
                            let mut prev_new_ticks = 0;
                            let mut frame = 0;
                            loop {
                                let ticks = (frame as f64 * ticks_per_sec / fps).round() as i64;
                                let new_ticks = filter.video_segment_suite.transform_node_time(clip_node, ticks)?;
                                if frame > 0 {
                                    let original_diff = ticks - prev_ticks;
                                    let new_diff = new_ticks - prev_new_ticks;
                                    let speed = (new_diff as f64 / original_diff as f64) * 100.0;
                                    if speed.abs() > 0.001 {
                                        speed_per_frame.push(speed);
                                    } else {
                                        break;
                                    }
                                } else {
                                    speed_per_frame.push(100.0);
                                }
                                prev_ticks = ticks;
                                prev_new_ticks = new_ticks;
                                frame += 1;
                                if frame > 5_000_000 { break; }
                            }
                            inst.stored.write().speed_per_frame = speed_per_frame;
                        }
                    }
                }

                let path = params.get_string(Params::ProjectPath).unwrap();
                if path.is_empty() {
                    return Ok(());
                }
                let instance_id = params.get_string(Params::InstanceId).unwrap();
                let disable_stretch = params.get_bool(Params::DisableStretch).unwrap();

                let out_w = params.get_f64(Params::OutputWidth).unwrap();
                let out_h = params.get_f64(Params::OutputHeight).unwrap();

                let key = format!("{path}{disable_stretch}{instance_id}");
                //log::info!("key {key}, out_size {out_w}x{out_h}");

                // log::info!("PremiereGPU::render! {pixel_format:?} in: {in_frame_data:?}, out: {out_frame_data:?}, stride: {in_stride}/{out_stride}, bounds: {in_bounds:?}/{out_bounds:?}, disable_stretch: {disable_stretch:?} path: {} instance_id: {instance_id:?} | time: {}", path, render_params.clip_time());

                //log::info!("{:?}", inst.stored);

                let base_inst = inst.gyroflow.as_mut().unwrap();
                base_inst.timeline_size = (render_params.render_width() as _, render_params.render_height() as _);

                if let Ok(stab) = base_inst.stab_manager(&mut params, &super::global_inst().gyroflow.manager_cache, (out_size.0 as _, out_size.1 as _), false) {
                    let fps = stab.params.read().fps;

                    let fps_ticks = inst.stored.read().media_fps_ticks;
                    let fps_ticks = if fps_ticks == 0 { ticks_per_sec / fps } else { fps_ticks as f64 };

                    // round the timestamp_us according to the fps, so it's never between frames and always points to a valid frame timestamp
                    let frame = render_params.clip_time() as f64 / fps_ticks;
                    let frame = if frame.fract() > 0.999 { frame.ceil() } else { frame.floor() };
                    let timestamp_us = (frame * (1_000_000.0 / fps)).round() as i64;

                    let src_size = (in_size.0 as usize, in_size.1 as usize, in_stride as usize);
                    let dest_size = (out_size.0 as usize, out_size.1 as usize, out_stride as usize);
                    // let src_rect = GyroflowPluginBase::get_center_rect(in_size.0 as usize, in_size.1 as usize, org_ratio);
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
                            BufferSource::MetalBuffer { buffer: in_ptr  as *mut metal::MTLBuffer, command_queue: filter.gpu_info.outCommandQueueHandle as *mut _ },
                            BufferSource::MetalBuffer { buffer: out_ptr as *mut metal::MTLBuffer, command_queue: std::ptr::null_mut() },
                            true
                        ),
                        pr::sys::PrGPUDeviceFramework_PrGPUDeviceFramework_OpenCL => (
                            BufferSource::OpenCL { texture: in_ptr,  queue: filter.gpu_info.outCommandQueueHandle },
                            BufferSource::OpenCL { texture: out_ptr, queue: std::ptr::null_mut() },
                            true
                        ),
                        _ => panic!("Invalid GPU framework")
                    };

                    let input_rotation = -params.get_f64(Params::InputRotation).unwrap() as f32;

                    let mut buffers = Buffers {
                        input:  BufferDescription { size: src_size,  rect: None,           data: buffers.0, rotation: Some(input_rotation), texture_copy: buffers.2 },
                        output: BufferDescription { size: dest_size, rect: Some(out_rect), data: buffers.1, rotation: None,                 texture_copy: buffers.2 }
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

pr::define_gpu_filter!(PremiereGPU);
