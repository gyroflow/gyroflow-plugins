// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2023 Adrian <adrian.eddy at gmail>

mod frei0r;
use frei0r::*;

use cstr::cstr;
use std::sync::{ Arc, atomic::AtomicBool };

use gyroflow_plugin_base::gyroflow_core::{ StabilizationManager, filesystem, stabilization::RGBA8, stabilization::Interpolation };
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
        (*info).num_params = 4;
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

                    if !path.ends_with(".gyroflow") {
                        if let Err(e) = inst.stab.load_video_file(&filesystem::path_to_url(&path), None) {
                            log::error!("An error occured: {e:?}");
                        }
                    } else {
                        if let Err(e) = inst.stab.import_gyroflow_file(&filesystem::path_to_url(&path), true, |_|(), Arc::new(AtomicBool::new(false))) {
                            log::error!("import_gyroflow_file error: {e:?}");
                        }
                    }

                    let video_size = inst.stab.params.read().video_size;

                    let org_ratio = video_size.0 as f64 / video_size.1 as f64;

                    let src_rect = GyroflowPluginBase::get_center_rect(inst.width, inst.height, org_ratio);
                    inst.stab.set_size(src_rect.2, src_rect.3);
                    inst.stab.set_output_size(inst.width, inst.height);

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
            _ => { }
        }
    }

    let _ = Box::into_raw(inst);
}
#[no_mangle]
extern "C" fn f0r_update(instance: f0r_instance_t, time: f64, inframe: *const u32, outframe: *mut u32) {
    if instance.is_null() { return; }
    let inst = unsafe { Box::from_raw(instance as *mut Instance) };

    let timestamp_us = (time * 1_000_000.0 * inst.time_scale).round() as i64;

    let org_ratio = {
        let params = inst.stab.params.read();
        params.video_size.0 as f64 / params.video_size.1 as f64
    };

    let src_size = (inst.width, inst.height, inst.width * 4);
    let src_rect = GyroflowPluginBase::get_center_rect(inst.width, inst.height, org_ratio);

    let inframe  = unsafe { std::slice::from_raw_parts_mut(inframe as *mut u8, inst.width * inst.height * 4) };
    let outframe = unsafe { std::slice::from_raw_parts_mut(outframe as *mut u8, inst.width * inst.height * 4) };

    let mut buffers = Buffers {
        input: BufferDescription {
            size: src_size,
            rect: Some(src_rect),
            data: BufferSource::Cpu { buffer: inframe },
            rotation: None,
            texture_copy: false
        },
        output: BufferDescription {
            size: src_size,
            rect: None,
            data: BufferSource::Cpu { buffer: outframe },
            rotation: None,
            texture_copy: false
        }
    };

    if let Err(e) = inst.stab.process_pixels::<RGBA8>(timestamp_us, &mut buffers) {
        log::debug!("process_pixels error: {e:?}");
    }

    let _ = Box::into_raw(inst);
}
