#![allow(dead_code)]
#![allow(non_camel_case_types)]

pub const FREI0R_MAJOR_VERSION: i32 = 1;
pub const FREI0R_MINOR_VERSION: i32 = 2;
pub const F0R_PLUGIN_TYPE_FILTER: i32 = 0;
pub const F0R_PLUGIN_TYPE_SOURCE: i32 = 1;
pub const F0R_PLUGIN_TYPE_MIXER2: i32 = 2;
pub const F0R_PLUGIN_TYPE_MIXER3: i32 = 3;
pub const F0R_COLOR_MODEL_BGRA8888: i32 = 0;
pub const F0R_COLOR_MODEL_RGBA8888: i32 = 1;
pub const F0R_COLOR_MODEL_PACKED32: i32 = 2;
pub const F0R_PARAM_BOOL: i32 = 0;
pub const F0R_PARAM_DOUBLE: i32 = 1;
pub const F0R_PARAM_COLOR: i32 = 2;
pub const F0R_PARAM_POSITION: i32 = 3;
pub const F0R_PARAM_STRING: i32 = 4;

/// The boolean type. The allowed range of values is [0, 1]. [0, 0.5[ is mapped to false and [0.5, 1] is mapped to true.
pub type f0r_param_bool = f64;
/// The double type. The allowed range of values is [0, 1].
pub type f0r_param_double = f64;
/// The string type. Zero terminated array of 8-bit values in utf-8 encoding
pub type f0r_param_string = *mut ::std::os::raw::c_char;
/// Transparent instance pointer of the frei0r effect.
pub type f0r_instance_t = *mut ::std::os::raw::c_void;
/// Transparent parameter handle.
pub type f0r_param_t = *mut ::std::os::raw::c_void;

/// The color type. All three color components are in the range [0, 1].
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct f0r_param_color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
/// The position type. Both position coordinates are in the range [0, 1].
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct f0r_param_position {
    pub x: f64,
    pub y: f64,
}
/// Similar to f0r_plugin_info_t, this structure is filled by the plugin for every parameter.
///
/// All strings are unicode, 0-terminated, and the encoding is utf-8.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct f0r_param_info {
    /// <The (short) name of the param
    pub name: *const ::std::os::raw::c_char,
    /// <The type (see the F0R_PARAM_* defines)
    pub type_: ::std::os::raw::c_int,
    /// <Optional explanation (can be 0)
    pub explanation: *const ::std::os::raw::c_char,
}

/// The f0r_plugin_info structure is filled in by the plugin to tell the application about its name, type, number of parameters and version.
///
/// An application should ignore (i.e. not use) frei0r effects that have unknown values in the plugin_type or color_model field.
/// It should also ignore effects with a too high frei0r_version.
///
/// This is necessary to be able to extend the frei0r spec (e.g. by adding new color models or plugin types) in a way that does not
/// result in crashes when loading effects that make use of these extensions into an older application.
///
/// All strings are unicode, 0-terminated, and the encoding is utf-8.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct f0r_plugin_info {
    /// < The (short) name of the plugin
    pub name: *const ::std::os::raw::c_char,
    /// < The plugin author
    pub author: *const ::std::os::raw::c_char,
    /// The plugin type, see PLUGIN_TYPE
    pub plugin_type: ::std::os::raw::c_int,
    /// < The color model used
    pub color_model: ::std::os::raw::c_int,
    /// < The frei0r major version this plugin is built for
    pub frei0r_version: ::std::os::raw::c_int,
    /// < The major version of the plugin
    pub major_version: ::std::os::raw::c_int,
    /// < The minor version of the plugin
    pub minor_version: ::std::os::raw::c_int,
    /// < The number of parameters of the plugin
    pub num_params: ::std::os::raw::c_int,
    /// < An optional explanation string
    pub explanation: *const ::std::os::raw::c_char,
}

extern "C" {
    /// f0r_init() is called once when the plugin is loaded by the application.
    /// \see f0r_deinit
    pub fn f0r_init() -> ::std::os::raw::c_int;

    /// f0r_deinit is called once when the plugin is unloaded by the application.
    /// \see f0r_init
    pub fn f0r_deinit();

    /// The f0r_plugin_info structure is filled in by the plugin to tell the application about its name, type, number of parameters and version.
    ///
    /// An application should ignore (i.e. not use) frei0r effects that have unknown values in the plugin_type or color_model field.
    /// It should also ignore effects with a too high frei0r_version.
    ///
    /// This is necessary to be able to extend the frei0r spec (e.g. by adding new color models or plugin types) in a way that does not
    /// result in crashes when loading effects that make use of these extensions into an older application.
    ///
    /// All strings are unicode, 0-terminated, and the encoding is utf-8.
    /// Is called once after init. The plugin has to fill in the values in info.
    ///
    /// \\param info Pointer to an info struct allocated by the application.
    pub fn f0r_get_plugin_info(info: *mut f0r_plugin_info);

    /// f0r_get_param_info is called by the application to query the type of each parameter.
    ///
    ///  \\param info is allocated by the application and filled by the plugin
    ///  \\param param_index the index of the parameter to be queried (from 0 to num_params-1)
    pub fn f0r_get_param_info(info: *mut f0r_param_info, param_index: ::std::os::raw::c_int);

    /// Constructor for effect instances. The plugin returns a pointer to its internal instance structure.
    ///
    ///  The resolution must be an integer multiple of 8, must be greater than 0 and be at most 2048 in both dimensions.
    ///  The plugin must set default values for all parameters in this function.
    ///
    ///  \\param width The x-resolution of the processed video frames
    ///  \\param height The y-resolution of the processed video frames
    ///  \\returns 0 on failure or a pointer != 0 on success
    ///
    ///  \\see f0r_destruct
    pub fn f0r_construct(width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint) -> f0r_instance_t;

    /// Destroys an effect instance.
    ///
    /// \\param instance The pointer to the plugins internal instance structure.
    ///
    /// \\see f0r_construct
    pub fn f0r_destruct(instance: f0r_instance_t);

    /// This function allows the application to set the parameter values of an effect instance. Validity of the parameter pointer is handled by the
    /// application thus the data must be copied by the effect.
    ///
    /// If the parameter type is of F0R_PARAM_STRING, then the caller should supply a pointer to f0r_param_string (char**). The plugin must
    /// copy the string and not assume it exists beyond the lifetime of the call.
    /// The reason a double pointer is requested when only a single is really needed is simply for API consistency.
    ///
    /// Furthermore, if an update event/signal is needed in a host application to notice when parameters have changed, this should be
    /// implemented inside its own update() call. The host application would presumably need to store the current value as well to see if
    /// it changes; to make this thread safe, it should store a copy of the current value in a struct which uses instance as a key.
    ///
    /// \\param instance the effect instance
    /// \\param param pointer to the parameter value
    /// \\param param_index index of the parameter
    ///
    /// \\see f0r_get_param_value
    pub fn f0r_set_param_value(
        instance: f0r_instance_t,
        param: f0r_param_t,
        param_index: ::std::os::raw::c_int,
    );

    /// This function allows the application to query the parameter values of an
    ///  effect instance.
    ///
    ///  If the parameter type is of F0R_PARAM_STRING, then the caller should
    ///  supply a pointer to f0r_param_string (char**). The plugin sets the
    ///  pointer to the address of its copy of the parameter value. Therefore,
    ///  the caller should not free the result. If the caller needs to modify
    ///  the value, it should make a copy of it and modify before calling
    ///  f0r_set_param_value().
    ///
    ///  \\param instance the effect instance
    ///  \\param param pointer to the parameter value
    ///  \\param param_index index of the parameter
    ///
    ///  \\see f0r_set_param_value
    pub fn f0r_get_param_value(
        instance: f0r_instance_t,
        param: f0r_param_t,
        param_index: ::std::os::raw::c_int,
    );

    /// This is where the core effect processing happens. The application calls it
    ///  after it has set the necessary parameter values.
    ///  inframe and outframe must be aligned to an integer multiple of 16 bytes
    ///  in memory.
    ///
    ///  This function should not alter the parameters of the effect in any
    ///  way (\\ref f0r_get_param_value should return the same values after a call
    ///  to \\ref f0r_update as before the call).
    ///
    ///  The function is responsible to restore the fpu state (e.g. rounding mode)
    ///  and mmx state if applicable before it returns to the caller.
    ///
    ///  The host mustn't call \\ref f0r_update for effects of type
    ///  \\ref F0R_PLUGIN_TYPE_MIXER2 and \\ref F0R_PLUGIN_TYPE_MIXER3.
    ///
    ///  \\param instance the effect instance
    ///  \\param time the application time in seconds but with subsecond resolution
    ///         (e.g. milli-second resolution). The resolution should be at least
    ///         the inter-frame period of the application.
    ///  \\param inframe the incoming video frame (can be zero for sources)
    ///  \\param outframe the resulting video frame
    ///
    ///  \\see f0r_update2
    pub fn f0r_update(instance: f0r_instance_t, time: f64, inframe: *const u32, outframe: *mut u32);

    /// For effects of type \ref F0R_PLUGIN_TYPE_SOURCE or \ref F0R_PLUGIN_TYPE_FILTER this method is optional. The \ref f0r_update
    ///  method must still be exported for these two effect types. If both are provided the behavior of them must be the same.
    ///
    ///  Effects of type \\ref F0R_PLUGIN_TYPE_MIXER2 or \\ref F0R_PLUGIN_TYPE_MIXER3 must provide the new \\ref f0r_update2 method.
    ///
    ///  \\param instance the effect instance
    ///  \\param time the application time in seconds but with subsecond resolution (e.g. milli-second resolution). The resolution should be at least the inter-frame period of the application.
    ///  \\param inframe1 the first incoming video frame (can be zero for sources)
    ///  \\param inframe2 the second incoming video frame (can be zero for sources and filters)
    ///  \\param inframe3 the third incoming video frame (can be zero for sources, filters and mixer2)
    ///  \\param outframe the resulting video frame
    ///
    ///  \\see f0r_update
    pub fn f0r_update2(
        instance: f0r_instance_t,
        time: f64,
        inframe1: *const u32,
        inframe2: *const u32,
        inframe3: *const u32,
        outframe: *mut u32,
    );
}
