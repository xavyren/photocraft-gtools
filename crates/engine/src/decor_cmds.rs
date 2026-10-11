//! Filter › Decor commands.

use photocraft_doc::{Layer, Size};
use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, parse_hex};
use crate::{EngineError, Result, Session};

const CMD: &str = "filter.decor.addBorder";
const MAX_BORDER_WIDTH: i32 = crate::image_cmds::MAX_CROP_SIDE / 2;

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn border_color(s: &Session, p: &Value) -> Result<[f32; 4]> {
    match p.get("color") {
        None => Ok(s.tools.foreground),
        Some(Value::String(value)) => parse_hex(value).ok_or_else(|| bad("`color` must be a hex color such as #rrggbb")),
        Some(_) => Err(bad("`color` must be a hex color such as #rrggbb")),
    }
}

fn add_border(s: &mut Session, p: &Value) -> Result<Value> {
    let color = border_color(s, p)?;
    let width = match p.get("width") {
        None => 32,
        Some(value) => value.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| bad("`width` must be a whole number of pixels"))?,
    };
    if width <= 0 || width > MAX_BORDER_WIDTH {
        return Err(bad(format!("`width` must be a whole number of pixels from 1 to {MAX_BORDER_WIDTH}")));
    }
    let twice_border = u32::try_from(width)
        .map_err(|_| bad("`width` is outside the supported pixel range"))?
        .checked_mul(2)
        .ok_or_else(|| bad("canvas dimensions overflow"))?;
    let expand = match p.get("expandCanvas") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err(bad("`expandCanvas` must be a boolean")),
    };
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let old_width = doc.size.width;
    let old_height = doc.size.height;
    let fmt = doc.pixel_format();
    let (new_width, new_height) = if expand {
        let w = old_width
            .checked_add(twice_border)
            .filter(|v| *v <= crate::image_cmds::MAX_CROP_SIDE as u32)
            .ok_or_else(|| bad(format!("border would exceed the {} px canvas-side limit", crate::image_cmds::MAX_CROP_SIDE)))?;
        let h = old_height
            .checked_add(twice_border)
            .filter(|v| *v <= crate::image_cmds::MAX_CROP_SIDE as u32)
            .ok_or_else(|| bad(format!("border would exceed the {} px canvas-side limit", crate::image_cmds::MAX_CROP_SIDE)))?;
        (w, h)
    } else {
        if old_width > crate::image_cmds::MAX_CROP_SIDE as u32 || old_height > crate::image_cmds::MAX_CROP_SIDE as u32 {
            return Err(bad(format!("canvas exceeds the {} px side limit", crate::image_cmds::MAX_CROP_SIDE)));
        }
        (old_width, old_height)
    };
    let new_width_i32 = i32::try_from(new_width).map_err(|_| bad("new canvas width is outside the 32-bit coordinate range"))?;
    let new_height_i32 = i32::try_from(new_height).map_err(|_| bad("new canvas height is outside the 32-bit coordinate range"))?;
    let area = |w: u32, h: u32| u64::from(w).saturating_mul(u64::from(h));
    let border_pixels = if expand {
        area(new_width, new_height).saturating_sub(area(old_width, old_height))
    } else {
        let inner_width = new_width.saturating_sub(twice_border);
        let inner_height = new_height.saturating_sub(twice_border);
        area(new_width, new_height).saturating_sub(area(inner_width, inner_height))
    };
    let border_bytes = border_pixels.saturating_mul(fmt.bytes_per_pixel().max(1) as u64);
    if border_bytes > crate::image_cmds::MAX_RESAMPLE_BYTES {
        let max_gib = crate::image_cmds::MAX_RESAMPLE_BYTES / (1 << 30);
        return Err(bad(format!("border exceeds the {max_gib} GiB raster pixel budget")));
    }
    let px = photocraft_raster::from_rgba(&fmt, color);
    let offset = if expand { width } else { 0 };
    let border_width = width;

    s.edit("Add Border", |doc, active| {
        if expand {
            crate::image_cmds::translate_doc(doc, CMD, offset, offset)?;
            doc.size = Size::new(new_width, new_height);
            crate::canvas_geom::refresh(doc, crate::canvas_geom::Refresh::Shapes);
            if let Some(background) = doc.layers.first_mut()
                && crate::extra_cmds::is_background(background)
            {
                crate::extra_cmds::unlock_background(background);
            }
        }

        let mut layer = Layer::raster(doc.next_layer_name("Border"), fmt);
        let surface = crate::pixels_mut(&mut layer)?;
        let canvas_width = new_width_i32;
        let canvas_height = new_height_i32;
        let top_end = border_width.min(canvas_height);
        let bottom_start = canvas_height.saturating_sub(border_width);
        let left_end = border_width.min(canvas_width);
        let right_start = canvas_width.saturating_sub(border_width);
        surface.fill_rect(Rect::new(0, 0, canvas_width, top_end), &px);
        surface.fill_rect(Rect::new(0, bottom_start, canvas_width, canvas_height), &px);
        if top_end < bottom_start {
            surface.fill_rect(Rect::new(0, top_end, left_end, bottom_start), &px);
            surface.fill_rect(Rect::new(right_start, top_end, canvas_width, bottom_start), &px);
        }

        let id = doc.insert_above(None, layer);
        *active = Some(id);
        Ok(id)
    })?;
    Ok(json!({"width": new_width, "height": new_height}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Add Border…",
        menu: &["Filter", "Decor"],
        shortcut: None,
        params: r##"{"color":color,"width":px=32,"expandCanvas":bool=true}"##,
        enabled: has_doc,
        run: add_border,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_doc::Document;
    use photocraft_geom::Rect;
    use serde_json::json;

    fn session(depth: u32) -> (Session, photocraft_doc::LayerId) {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 16, "height": 12, "depth": depth})).unwrap();
        let original = photocraft_doc::LayerId(s.execute("layer.new.layer", json!({"name": "Original"})).unwrap()["layer"].as_u64().unwrap());
        s.execute("select.rect", json!({"x": 4, "y": 3, "width": 4, "height": 3})).unwrap();
        s.execute("edit.fill", json!({"contents": "color", "color": "#ff0000"})).unwrap();
        s.execute("select.deselect", json!({})).unwrap();
        (s, original)
    }

    fn doc(s: &Session) -> &Document {
        &s.active().unwrap().doc
    }

    fn pixel(layer: &Layer, x: i32, y: i32) -> Vec<f32> {
        layer.surface().unwrap().read_region(Rect::new(x, y, x + 1, y + 1))
    }

    #[test]
    fn expanded_border_preserves_and_recentres_the_original_content() {
        for depth in [8, 16, 32] {
            let (mut s, original) = session(depth);
            let original_count = doc(&s).layers.len();
            let past = s.active().unwrap().history.past_len();
            s.tools.foreground = [0.0, 1.0, 0.0, 1.0];
            let result = s.execute(CMD, json!({"width": 2, "expandCanvas": true})).unwrap();
            assert_eq!(result, json!({"width": 20, "height": 16}), "{depth}");

            let d = doc(&s);
            assert_eq!(d.size, Size::new(20, 16), "{depth}");
            assert_eq!(d.layers.len(), original_count + 1, "{depth}");
            assert_eq!(s.active().unwrap().history.past_len(), past + 1, "{depth}");
            assert_eq!(d.layers.last().map(|l| l.name.as_str()), Some("Border 1"), "{depth}");
            assert_eq!(d.layers.first().map(|l| l.name.as_str()), Some("Layer 0"), "{depth}: expanded Background is unlocked");
            assert_eq!(pixel(&d.layers[0], 0, 0), vec![0.0, 0.0, 0.0, 0.0], "{depth}: old Background stays transparent outside its translated bounds");
            let border = d.layers.last().unwrap();
            assert_eq!(pixel(border, 10, 1), vec![0.0, 1.0, 0.0, 1.0], "{depth}: border pixel");
            assert_eq!(pixel(border, 5, 5), vec![0.0, 0.0, 0.0, 0.0], "{depth}: transparent middle");
            let original_layer = d.layer(original).unwrap();
            assert_eq!(pixel(original_layer, 6, 5), vec![1.0, 0.0, 0.0, 1.0], "{depth}: original moved by the border width");

            assert!(s.undo(), "{depth}: undo");
            assert_eq!(doc(&s).size, Size::new(16, 12), "{depth}: undo restores canvas");
            assert_eq!(doc(&s).layer_count(), original_count, "{depth}: undo removes border");
            assert!(s.redo(), "{depth}: redo");
            assert_eq!(doc(&s).size, Size::new(20, 16), "{depth}: redo reapplies expansion");
        }
    }

    #[test]
    fn inward_border_does_not_resize_or_cover_the_interior() {
        for depth in [8, 16, 32] {
            let (mut s, original) = session(depth);
            let original_count = doc(&s).layers.len();
            s.execute(CMD, json!({"color": "#0000ff", "width": 2, "expandCanvas": false})).unwrap();

            let d = doc(&s);
            assert_eq!(d.size, Size::new(16, 12), "{depth}");
            assert_eq!(d.layers.len(), original_count + 1, "{depth}");
            let border = d.layers.last().unwrap();
            assert_eq!(pixel(border, 1, 6), vec![0.0, 0.0, 1.0, 1.0], "{depth}: inward edge");
            assert_eq!(pixel(border, 8, 6), vec![0.0, 0.0, 0.0, 0.0], "{depth}: transparent interior");
            assert_eq!(pixel(d.layer(original).unwrap(), 5, 4), vec![1.0, 0.0, 0.0, 1.0], "{depth}: original pixels untouched");
        }
    }

    #[test]
    fn invalid_border_params_leave_document_unchanged() {
        let (mut s, _) = session(8);
        let before = doc(&s).clone();
        let past = s.active().unwrap().history.past_len();
        for params in [
            json!({"width": 0}),
            json!({"width": -1}),
            json!({"width": 1.5}),
            json!({"width": 32, "color": "not-hex"}),
            json!({"width": 150_001}),
            json!({"width": 2, "expandCanvas": "yes"}),
        ] {
            assert!(s.execute(CMD, params.clone()).is_err(), "expected invalid params to fail: {params}");
            assert_eq!(doc(&s), &before);
            assert_eq!(s.active().unwrap().history.past_len(), past);
        }
    }

    #[test]
    fn oversized_border_is_rejected_before_editing() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 1, "height": 1})).unwrap();
        let before = doc(&s).clone();
        let past = s.active().unwrap().history.past_len();

        assert!(s.execute(CMD, json!({"width": 25_000, "expandCanvas": true})).is_err());
        assert_eq!(doc(&s), &before);
        assert_eq!(s.active().unwrap().history.past_len(), past);
    }
}
