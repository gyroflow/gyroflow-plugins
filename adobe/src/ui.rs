use super::*;
use std::sync::OnceLock;

pub struct PngImage {
    pub width: usize,
    pub height: usize,
    pub line_size: usize,
    pub data: Vec<u8>,
}
impl PngImage {
    pub fn new(png_bytes: &[u8]) -> Self {
        let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut data = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut data).unwrap();
        if data.len() != info.buffer_size() {
            data.resize(info.buffer_size(), 0)
        }
        Self::rgba_to_bgra(&mut data);
        Self {
            width: info.width as _,
            height: info.height as _,
            line_size: info.line_size as _,
            data,
        }
    }
    pub fn rgba_to_bgra(data: &mut [u8]) {
        for chunk in data.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
    }
}

pub fn draw(_in_data: &ae::InData, params: &mut ae::Parameters<Params>, event: &mut ae::EventExtra, inst: &mut CrossThreadInstance) -> Result<(), ae::Error> {
    if event.effect_area() == ae::EffectArea::Control {
        let current_frame = event.current_frame();

        let drawbot = event.context_handle().drawing_reference()?;
        let supplier = drawbot.supplier()?;
        let surface = drawbot.surface()?;

        // Fill the background
        static BG_COLOR: OnceLock<ae::drawbot::ColorRgba> = OnceLock::new();
        surface.paint_rect(BG_COLOR.get_or_init(acquire_background_color), &ae::drawbot::RectF32 {
            left:   current_frame.left     as f32,
            top:    current_frame.top      as f32,
            width:  current_frame.width()  as f32,
            height: current_frame.height() as f32 + 1.0,
        })?;

        // Draw logo
        if event.param_index() == params.index(Params::Logo).unwrap_or_default() {
            static PNG: OnceLock<PngImage> = OnceLock::new();
            let png = PNG.get_or_init(|| PngImage::new(include_bytes!("../logo_white.png")));

            if let Ok(img) = supplier.new_image_from_buffer(png.width, png.height, png.line_size, drawbot::PixelLayout::Bgra32Straight, &png.data) {
                let origin = drawbot::PointF32 {
                    x: current_frame.left as f32 + (current_frame.width() as f32 - png.width as f32) / 2.0,
                    y: current_frame.top as f32,
                };
                surface.draw_image(&img, &origin, 1.0)?;
            }
        }

        // Draw status
        if event.param_index() == params.index(Params::Status).unwrap_or_default() {
            let status = get_str(params, inst, Params::Status)?;
            let proj   = get_str2(inst, Params::LoadedProject).unwrap_or("---".to_owned());
            let lens   = get_str2(inst, Params::LoadedLens).unwrap_or("---".to_owned());
            let preset = get_str2(inst, Params::LoadedPreset).unwrap_or_default();

            if !status.is_empty() {
                let font = supplier.new_default_font(supplier.default_font_size()? * 0.9)?;
                let text_color = if status == "OK" {
                    ae::drawbot::ColorRgba { red: 0.22, green: 0.86, blue: 0.1, alpha: 1.0 } // Green
                } else if status == "Calculating..." {
                    ae::drawbot::ColorRgba { red: 0.92, green: 0.57, blue: 0.08, alpha: 1.0 } // Yellow
                } else if status == "---" {
                    ae::drawbot::ColorRgba { red: 0.8, green: 0.8, blue: 0.8, alpha: 1.0 } // White
                } else {
                    ae::drawbot::ColorRgba { red: 0.95, green: 0.15, blue: 0.15, alpha: 1.0 } // Red
                };
                let string_brush = supplier.new_brush(&text_color)?;
                let mut origin = ae::drawbot::PointF32 {
                    x: current_frame.left as f32,
                    y: current_frame.top as f32 + 10.0,
                };

                surface.draw_string(&string_brush, &font, &status, &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;

                let string_brush = supplier.new_brush(&ae::drawbot::ColorRgba { red: 0.8, green: 0.8, blue: 0.8, alpha: 1.0 })?;
                origin.y += 15.0;
                surface.draw_string(&string_brush, &font, &format!("Project: {proj}"),  &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                origin.y += 15.0;
                surface.draw_string(&string_brush, &font, &format!("Lens: {lens}"),     &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                if !preset.is_empty() {
                    origin.y += 15.0;
                    surface.draw_string(&string_brush, &font, &format!("Preset: {preset}"), &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                }
            }
        }
    }
    event.set_event_out_flags(ae::EventOutFlags::HANDLED_EVENT);

    Ok(())
}

pub fn acquire_background_color() -> ae::drawbot::ColorRgba {
    const MAX_SHORT_COLOR: f32 = 65535.0;
    const INV_SIXTY_FIVE_K: f32 = 1.0 / MAX_SHORT_COLOR;

    let bg = ae::pf::suites::App::new()
        .and_then(|x| x.bg_color())
        .unwrap_or(ae::sys::PF_App_Color { red: 9830, green: 9830, blue: 9830 });
    ae::drawbot::ColorRgba {
        red:   bg.red   as f32 * INV_SIXTY_FIVE_K,
        green: bg.green as f32 * INV_SIXTY_FIVE_K,
        blue:  bg.blue  as f32 * INV_SIXTY_FIVE_K,
        alpha: 1.0,
    }
}

fn get_str(params: &ae::Parameters<Params>, inst: &mut CrossThreadInstance, key: Params) -> Result<String, ae::Error> {
    let mut value = params.get(key)?.as_arbitrary()?.value::<ArbString>().map(|x| x.get().to_owned()).unwrap_or_default();
    if value.is_empty() {
        if let Ok(v2) = get_str2(inst, key) {
            value = v2;
        }
    }
    Ok(value)
}
fn get_str2(inst: &mut CrossThreadInstance, key: Params) -> Result<String, ae::Error> {
    let _self = inst.get().unwrap();
    if let Some(_self) = _self.try_read_for(std::time::Duration::from_millis(300)) {
        if let Some(v) = _self.stored.read().pending_params_str.get(&key) {
            return Ok(v.clone());
        }
    };
    Err(ae::Error::InvalidParms)
}
