//! `margins.py`: `MarginsRenderEngine` (`:25`), in its `"print"` mode (the mode
//! `PrintPayloadRenderEngine` uses, `print_payload.py:11`).

use super::super::constants::Direction;
use super::super::pillow::{Image, Mode};
use super::render_engine::RenderError;

/// `is_env_var_true("LABELLE_DEV_MODE_NO_MARGINS")` (`env_config.py:4`, `:9`).
fn is_dev_mode_no_margins() -> bool {
    std::env::var("LABELLE_DEV_MODE_NO_MARGINS").is_ok_and(|v| matches!(v.to_lowercase().as_str(), "1" | "true"))
}

/// Python's `round(x)` of a float: halves to the even one.
fn py_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

pub struct MarginsRenderEngine {
    justify: Direction,
    visible_horizontal_margin_px: f64,
    labeler_horizontal_margin_px: f64,
    max_width_px: Option<f64>,
    min_width_px: f64,
}

impl MarginsRenderEngine {
    /// `__init__` (`margins.py:24`), `mode="print"`.
    pub fn new(
        justify: Direction,
        visible_horizontal_margin_px: f64,
        labeler_margin_px: (f64, f64),
        max_width_px: Option<f64>,
        min_width_px: Option<f64>,
    ) -> MarginsRenderEngine {
        let (labeler_horizontal_margin_px, labeler_vertical_margin_px) = labeler_margin_px;
        assert!(visible_horizontal_margin_px >= 0.0);
        assert!(labeler_horizontal_margin_px >= 0.0);
        assert!(labeler_vertical_margin_px >= 0.0);
        assert!(max_width_px.is_none_or(|m| m == 0.0 || m >= 0.0));
        let min_width_px = min_width_px.unwrap_or(0.0);
        assert!(min_width_px >= 0.0);
        if is_dev_mode_no_margins() {
            MarginsRenderEngine {
                justify,
                visible_horizontal_margin_px: 0.0,
                labeler_horizontal_margin_px: 0.0,
                max_width_px,
                min_width_px: 0.0,
            }
        } else {
            MarginsRenderEngine {
                justify,
                visible_horizontal_margin_px,
                labeler_horizontal_margin_px,
                max_width_px,
                min_width_px,
            }
        }
    }

    /// `_calculate_visible_width` (`margins.py:57`).
    fn calculate_visible_width(&self, payload_width_px: usize) -> Result<f64, RenderError> {
        let minimal_label_width_px = payload_width_px as f64 + self.visible_horizontal_margin_px * 2.0;
        // `if self.max_width_px is not None and ...`: a max of 0 is a value here (`is not None`).
        if let Some(max) = self.max_width_px
            && minimal_label_width_px > max
        {
            return Err(RenderError::BitmapTooBig(minimal_label_width_px, max));
        }
        if self.min_width_px > minimal_label_width_px {
            Ok(self.min_width_px)
        } else {
            Ok(minimal_label_width_px)
        }
    }

    /// `render_with_meta` (`margins.py:73`) in print mode: the bitmap of the label, the payload
    /// pasted on blank tape. Gives the bitmap and the horizontal offset it used.
    pub fn render_with_meta(&self, payload_bitmap: &Image) -> Result<(Image, f64), RenderError> {
        let payload_width_px = payload_bitmap.width;
        let label_width_px = self.calculate_visible_width(payload_width_px)?;
        let padding_px = label_width_px - payload_width_px as f64; // sum of margins from both sides

        let mut horizontal_offset_px = match self.justify {
            Direction::Left => self.visible_horizontal_margin_px,
            Direction::Center => padding_px / 2.0,
            Direction::Right => padding_px - self.visible_horizontal_margin_px,
        };
        assert!(horizontal_offset_px >= self.visible_horizontal_margin_px);

        // In print mode: print head is already in offset from label's edge under the cutter
        // (the long comment of margins.py:94-109 explains the cutter).
        horizontal_offset_px -= self.labeler_horizontal_margin_px;
        // no need to add vertical margins to bitmap
        let vertical_offset_px = 0.0;
        let bitmap_height = payload_bitmap.height as f64;

        let mut bitmap = Image::new(Mode::One, label_width_px.ceil() as usize, bitmap_height.ceil() as usize);
        bitmap
            .paste(payload_bitmap, py_round(horizontal_offset_px), py_round(vertical_offset_px))
            .map_err(RenderError::Pillow)?;
        Ok((bitmap, horizontal_offset_px))
    }
}
