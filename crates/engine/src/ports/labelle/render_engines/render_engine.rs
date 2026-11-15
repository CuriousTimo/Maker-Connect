//! `render_engine.py` (and `render_engines/exceptions.py`): the exceptions the engines raise.

/// `RenderEngineException` and its subclasses.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// `NoContentError` (`exceptions.py`): its message is empty.
    NoContent,
    /// `PicturePathDoesNotExist`.
    PicturePathDoesNotExist(String),
    /// `UnidentifiedImageFileError`: the message of Pillow's `UnidentifiedImageError`.
    UnidentifiedImageFile(String),
    /// `BitmapTooBigError` (`margins.py:17`): `width_px: {w}, max_width_px: {max}`.
    BitmapTooBig(f64, f64),
    /// What Pillow raises (`ValueError`, `OSError`, ...), as its message.
    Pillow(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoContent => Ok(()),
            RenderError::PicturePathDoesNotExist(m)
            | RenderError::UnidentifiedImageFile(m)
            | RenderError::Pillow(m) => f.write_str(m),
            RenderError::BitmapTooBig(width_px, max_width_px) => {
                write!(f, "width_px: {width_px:?}, max_width_px: {max_width_px:?}")
            }
        }
    }
}
