//! `picture.py`: `PictureRenderEngine` (`picture.py:25`).

use super::super::pillow::Image;
use super::super::pillow::open::{OpenError, open};
use super::render_context::RenderContext;
use super::render_engine::RenderError;
use std::path::PathBuf;

pub struct PictureRenderEngine {
    picture_path: PathBuf,
}

impl PictureRenderEngine {
    /// `__init__` (`picture.py:26`).
    pub fn new(picture_path: &str) -> Result<PictureRenderEngine, RenderError> {
        if picture_path.is_empty() {
            return Err(RenderError::NoContent);
        }
        let picture_path_buf = PathBuf::from(picture_path);
        if !picture_path_buf.is_file() {
            return Err(RenderError::PicturePathDoesNotExist(format!(
                "Picture path does not exist: {picture_path}"
            )));
        }
        Ok(PictureRenderEngine {
            picture_path: picture_path_buf,
        })
    }

    /// `render` (`picture.py:36`).
    pub fn render(&self, context: &RenderContext) -> Result<Image, RenderError> {
        let height_px = context.height_px;
        let bytes =
            std::fs::read(&self.picture_path).map_err(|e| RenderError::Pillow(e.to_string()))?;
        let img = open(&bytes, &self.picture_path.to_string_lossy()).map_err(|e| match e {
            // `except UnidentifiedImageError as e: raise UnidentifiedImageFileError(e) from e`
            OpenError::Unidentified(m) => RenderError::UnidentifiedImageFile(m),
            OpenError::Other(m) => RenderError::Pillow(m),
        })?;
        let mut img = img;
        if img.height as i64 > height_px {
            let ratio = height_px as f64 / img.height as f64;
            let width = (img.width as f64 * ratio).ceil() as usize;
            img = img
                .resize((width, height_px as usize))
                .map_err(RenderError::Pillow)?;
        }
        let img = img.convert_l().map_err(RenderError::Pillow)?;
        img.invert()
            .and_then(|i| i.convert_one())
            .map_err(RenderError::Pillow)
    }
}
