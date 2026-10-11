// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2023 Adrian <adrian.eddy at gmail>

mod frei0r;
use frei0r::*;

use cstr::cstr;
use std::sync::{ Arc, atomic::AtomicBool };

use gyroflow_plugin_base::gyroflow_core::{ self, StabilizationManager, filesystem, stabilization::RGBA8, stabilization::Interpolation };
use gyroflow_plugin_base::gyroflow_core::gpu::{ BufferDescription, Buffers, BufferSource };
use gyroflow_plugin_base::GyroflowPluginBase;

#[derive(Default)]
struct Instance {
    width: usize,
    height: usize,
    stab: StabilizationManager,
    // Params
    path: String,
    smoothness: f64,
    stab_overview: bool,
    time_scale: f64,
    time_offset: f64,
    // How the clip is turned in the frame, and whether the stabilized image stays where the clip is (see `GyroflowPluginBase::render_geometry`)
    input_rotation: f64,
    keep_clip_frame: bool,
}

#[no_mangle] extern "C" fn f0r_init() -> ::std::os::raw::c_int { 1 }
#[no_mangle] extern "C" fn f0r_deinit() { }

#[no_mangle]
extern "C" fn f0r_get_plugin_info(info: *mut f0r_plugin_info) {
    unsafe {
        (*info).name = cstr!("Gyroflow").as_ptr();
        (*info).author = cstr!("AdrianEddy").as_ptr();
        (*info).plugin_type = F0R_PLUGIN_TYPE_FILTER;
        (*info).color_model = F0R_COLOR_MODEL_PACKED32;
        (*info).frei0r_version = FREI0R_MAJOR_VERSION;
        (*info).major_version = 0;
        (*info).minor_version = 1;
        (*info).num_params = 5;
        (*info).explanation = cstr!("Gyroflow video stabilization").as_ptr();
    }
}
#[no_mangle]
extern "C" fn f0r_get_param_info(info: *mut f0r_param_info, index: ::std::os::raw::c_int) {
    unsafe {
        match index {
            0 => {
                (*info).name = cstr!("Project").as_ptr();
                (*info).type_ = F0R_PARAM_STRING;
                (*info).explanation = cstr!("Project file or video").as_ptr();
            },
            1 => {
                (*info).name = cstr!("Smoothness").as_ptr();
                (*info).type_ = F0R_PARAM_DOUBLE;
                (*info).explanation = cstr!("Smoothness").as_ptr();
            },
            2 => {
                (*info).name = cstr!("Overview").as_ptr();
                (*info).type_ = F0R_PARAM_BOOL;
                (*info).explanation = cstr!("Stabilization overview").as_ptr();
            },
            3 => {
                (*info).name = cstr!("TimestampScale").as_ptr();
                (*info).type_ = F0R_PARAM_DOUBLE;
                (*info).explanation = cstr!("Scale for the input timestamp").as_ptr();
            },
            4 => {
                (*info).name = cstr!("TimestampOffset").as_ptr();
                (*info).type_ = F0R_PARAM_DOUBLE;
                (*info).explanation = cstr!("Offset for the input timestamp in seconds").as_ptr();
            }
            _ => { }
        }
    }
}
#[no_mangle]
extern "C" fn f0r_construct(width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint) -> f0r_instance_t {
    let stab = StabilizationManager::default();
    {
        let mut stab = stab.stabilization.write();
        stab.share_wgpu_instances = true;
        stab.interpolation = Interpolation::Lanczos4;
    }

    let id = Box::new(Instance { width: width as usize, height: height as usize, stab, time_scale: 1.0, ..Default::default() });
    Box::into_raw(id) as f0r_instance_t
}
#[no_mangle]
extern "C" fn f0r_destruct(instance: f0r_instance_t) {
    if instance.is_null() { return; }
    unsafe {
        let _ = Box::from_raw(instance as *mut Instance);
    }
}
#[no_mangle]
extern "C" fn f0r_set_param_value(instance: f0r_instance_t, param: f0r_param_t, index: ::std::os::raw::c_int) {
    if instance.is_null() { return; }
    let mut inst = unsafe { Box::from_raw(instance as *mut Instance) };
    unsafe {
        match index {
            0 => { // Project file
                let path = std::ffi::CStr::from_ptr(*(param as *mut *mut std::ffi::c_char)).to_string_lossy().to_owned()
                    .replace("_DRIVE_SEP_", ":/")
                    .replace("_DIR_SEP_", "/");

                if path != inst.path {
                    inst.path = path.clone();

                    // Shotcut (MLT) shows videos turned by their rotation metadata
                    let mut metadata_rotation = 0;
                    if !path.ends_with(".gyroflow") {
                        let url = filesystem::path_to_url(&path);
                        if let Ok(mut file) = filesystem::open_file(&url, false, false) {
                            let filesize = file.size;
                            match inst.stab.load_video_file(file.get_file(), filesize, &url, None, true) {
                                Ok(md) => {
                                    metadata_rotation = md.rotation;
                                    // The output the right way up too, like the app does
                                    inst.stab.params.write().video_rotation = GyroflowPluginBase::input_rotation_from_metadata(md.rotation);
                                },
                                Err(e) => log::error!("An error occured: {e:?}")
                            }
                        };
                    } else {
                        if let Err(e) = inst.stab.import_gyroflow_file(&filesystem::path_to_url(&path), true, |_|(), Arc::new(AtomicBool::new(false)), true) {
                            log::error!("import_gyroflow_file error: {e:?}");
                        } else {
                            // Project gyro transforms (including LPF) are restored after the
                            // embedded motion data is loaded, so apply them before smoothing.
                            inst.stab.recompute_gyro();
                        }
                        // The output rotation is the project's
                        let url = inst.stab.input_file.read().url.clone();
                        if let Ok(mut file) = filesystem::open_file(&url, false, false) {
                            let filesize = file.size;
                            if let Ok(md) = gyroflow_core::util::get_video_metadata(file.get_file(), filesize, &url) {
                                metadata_rotation = md.rotation;
                            }
                        }
                    }
                    inst.input_rotation = GyroflowPluginBase::input_rotation_from_metadata(metadata_rotation);

                    // The clip is fitted in the frame, turned (`render_geometry` finds it there). The frame Gyroflow stabilizes
                    // stays the video's own size: lens metadata in pixels (Sony, Canon) is of that size, so scaling it to the
                    // clip's size in the frame made those lenses meaningless and the output black
                    let video_size = inst.stab.params.read().size;
                    let clip = GyroflowPluginBase::rotated_size(video_size, inst.input_rotation);
                    // The stabilized image fills the frame, except that a vertical clip in a horizontal frame keeps its pillarbox
                    // (see `GyroflowPluginBaseInstance::keep_clip_frame`)
                    inst.keep_clip_frame = clip.1 > clip.0 && inst.width > inst.height;
                    if inst.keep_clip_frame {
                        inst.stab.set_output_size(clip.0, clip.1);
                    } else {
                        inst.stab.set_output_size(inst.width, inst.height);
                    }

                    inst.stab.invalidate_smoothing();
                    inst.stab.recompute_blocking();
                }
            },
            1 => { // Smoothness
                let smoothness = *(param as *mut f64);
                if (smoothness - inst.smoothness).abs() > 0.001 {
                    inst.smoothness = smoothness;

                    inst.stab.smoothing.write().current_mut().set_parameter("smoothness", inst.smoothness);
                    inst.stab.invalidate_smoothing();
                    inst.stab.recompute_blocking();
                }
            },
            2 => { // Stabilization overview
                let overview = *(param as *mut f64) > 0.5;
                if overview != inst.stab_overview {
                    inst.stab_overview = overview;
                    inst.stab.set_fov_overview(inst.stab_overview);
                    inst.stab.recompute_undistortion();
                }
            },
            3 => { // Timestamp scale
                inst.time_scale = *(param as *mut f64);
            },
            4 => { // Timestamp offset
                inst.time_offset = *(param as *mut f64);
            },
            _ => { }
        }
    }

    let _ = Box::into_raw(inst);
}
#[no_mangle]
extern "C" fn f0r_get_param_value(instance: f0r_instance_t, param: f0r_param_t, index: ::std::os::raw::c_int) {
    if instance.is_null() { return; }
    let inst = unsafe { Box::from_raw(instance as *mut Instance) };
    unsafe {
        match index {
            0 => { // Project file
                *(param as *mut f0r_param_string) = std::ffi::CString::new(inst.path.clone()).unwrap().into_raw();
            },
            1 => { // Smoothness
                *(param as *mut f64) = inst.smoothness;
            },
            2 => { // Stabilization overview
                *(param as *mut f64) = if inst.stab_overview { 1.0 } else { 0.0 };
            },
            3 => { // Timestamp scale
                *(param as *mut f64) = inst.time_scale;
            },
            4 => { // Timestamp offset
                *(param as *mut f64) = inst.time_offset;
            },
            _ => { }
        }
    }

    let _ = Box::into_raw(inst);
}
#[no_mangle]
extern "C" fn f0r_update(instance: f0r_instance_t, time: f64, inframe: *const u32, outframe: *mut u32) {
    if instance.is_null() { return; }
    let inst = unsafe { Box::from_raw(instance as *mut Instance) };

    let timestamp_us = ((time * inst.time_scale + inst.time_offset) * 1_000_000.0).round() as i64;

    let video_size = inst.stab.params.read().size;
    let src_size = (inst.width, inst.height, inst.width * 4);
    let geometry = GyroflowPluginBase::render_geometry(video_size, inst.input_rotation, (inst.width, inst.height), (inst.width, inst.height), false, inst.keep_clip_frame, false);

    let inframe  = unsafe { std::slice::from_raw_parts_mut(inframe as *mut u8, inst.width * inst.height * 4) };
    let outframe = unsafe { std::slice::from_raw_parts_mut(outframe as *mut u8, inst.width * inst.height * 4) };

    let mut buffers = Buffers {
        input: BufferDescription {
            size: src_size,
            rect: Some(geometry.src_rect),
            data: BufferSource::Cpu { buffer: inframe },
            rotation: Some(geometry.kernel_rotation as f32),
            texture_copy: false
        },
        output: BufferDescription {
            size: src_size,
            rect: geometry.out_rect,
            data: BufferSource::Cpu { buffer: outframe },
            rotation: None,
            texture_copy: false
        }
    };

    if let Err(e) = inst.stab.process_pixels::<RGBA8>(timestamp_us, None, &mut buffers) {
        log::debug!("process_pixels error: {e:?}");
    }

    let _ = Box::into_raw(inst);
}
