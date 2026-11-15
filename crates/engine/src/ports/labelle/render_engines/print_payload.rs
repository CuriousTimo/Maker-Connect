//! `print_payload.py`: `PrintPayloadRenderEngine` (`:12`): the payload's engine inside a
//! `MarginsRenderEngine` in `"print"` mode.

use super::super::constants::Direction;
use super::super::pillow::Image;
use super::margins::MarginsRenderEngine;
use super::render_engine::RenderError;

pub struct PrintPayloadRenderEngine {
    render_engine: MarginsRenderEngine,
}

impl PrintPayloadRenderEngine {
    /// `__init__` (`print_payload.py:12`).
    pub fn new(
        justify: Direction,
        visible_horizontal_margin_px: f64,
        labeler_margin_px: (f64, f64),
        max_width_px: Option<f64>,
        min_width_px: Option<f64>,
    ) -> PrintPayloadRenderEngine {
        PrintPayloadRenderEngine {
            render_engine: MarginsRenderEngine::new(
                justify,
                visible_horizontal_margin_px,
                labeler_margin_px,
                max_width_px,
                min_width_px,
            ),
        }
    }

    /// `render_with_meta` (`print_payload.py:35`): the margins engine's, on the payload's bitmap.
    pub fn render_with_meta(&self, payload_bitmap: &Image) -> Result<(Image, f64), RenderError> {
        self.render_engine.render_with_meta(payload_bitmap)
    }
}
