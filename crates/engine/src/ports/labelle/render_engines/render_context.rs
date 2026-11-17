//! `render_context.py`: `RenderContext`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderContext {
    pub height_px: i64,
    pub preview_show_margins: bool,
    pub background_color: &'static str,
    pub foreground_color: &'static str,
}

impl RenderContext {
    /// `RenderContext(background_color="white", foreground_color="black", height_px=...,
    /// preview_show_margins=False)`, as `cli.py:594` makes it.
    pub fn new(height_px: i64) -> RenderContext {
        RenderContext {
            height_px,
            preview_show_margins: false,
            background_color: "white",
            foreground_color: "black",
        }
    }
}
