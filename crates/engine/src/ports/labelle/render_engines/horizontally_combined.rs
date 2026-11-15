//! `horizontally_combined.py`: `HorizontallyCombinedRenderEngine` (`:15`). The print path
//! gives it the one engine of a picture label; with one bitmap it returns that bitmap as it is
//! (`:31`). More engines would be text, QR and barcode ones: not ported.

use super::super::pillow::Image;

/// `render` (`horizontally_combined.py:24`) for the bitmaps of the engines: with one, itself.
pub fn render(mut bitmaps: Vec<Image>) -> Result<Image, String> {
    if bitmaps.len() == 1 {
        return Ok(bitmaps.remove(0));
    }
    Err("HorizontallyCombinedRenderEngine of several engines is not ported".to_owned())
}
