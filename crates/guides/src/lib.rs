//! Step-by-step guides (wiki: Design: Guides, issue #210): a guide has steps, a step has lines and
//! photos, a line has one style (a marking colour, or caution or note) and a level (0, or 1 for a
//! sub-line). This crate holds the data, the checks on what is saved, and a step as HTML. Storage,
//! photos, pages and the editor are the hub's.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// The most steps a guide has, lines a step has, photos a step has: limits of our editor.
pub const MAX_STEPS: usize = 200;
pub const MAX_LINES: usize = 40;
pub const MAX_PHOTOS: usize = 6;
pub const MAX_TITLE: usize = 120;
pub const MAX_TEXT: usize = 1000;

/// A guide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub title: String,
    /// 1 (very easy) to 5 (very difficult).
    pub difficulty: u8,
    /// A photo's name (see [`photo_name_ok`]); none: the first step's first photo.
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// One step: one action, its lines and its photos (the first is shown large).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub title: String,
    #[serde(default)]
    pub lines: Vec<Line>,
    #[serde(default)]
    pub photos: Vec<String>,
}

/// One line of a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Text with only bold, italic and links (cleaned by [`check`]).
    pub text: String,
    pub style: Style,
    /// 0, or 1: a sub-line under the line above.
    #[serde(default)]
    pub level: u8,
}

/// A line's style: a marking colour (the same colour as its marking in the photo), or an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Black,
    Red,
    Orange,
    Yellow,
    Green,
    LightBlue,
    Blue,
    Violet,
    Caution,
    Note,
}

impl Style {
    pub const ALL: [Style; 10] = [
        Style::Black,
        Style::Red,
        Style::Orange,
        Style::Yellow,
        Style::Green,
        Style::LightBlue,
        Style::Blue,
        Style::Violet,
        Style::Caution,
        Style::Note,
    ];

    /// The name in the data and in the CSS class `gl-<name>`.
    pub fn name(self) -> &'static str {
        match self {
            Style::Black => "black",
            Style::Red => "red",
            Style::Orange => "orange",
            Style::Yellow => "yellow",
            Style::Green => "green",
            Style::LightBlue => "light_blue",
            Style::Blue => "blue",
            Style::Violet => "violet",
            Style::Caution => "caution",
            Style::Note => "note",
        }
    }
}

/// The five levels of difficulty, by number.
pub const DIFFICULTY: [&str; 5] = [
    "Very easy",
    "Easy",
    "Moderate",
    "Difficult",
    "Very difficult",
];

/// The name of a level of difficulty (1-5).
pub fn difficulty_name(level: u8) -> &'static str {
    DIFFICULTY[usize::from(level.clamp(1, 5)) - 1]
}

/// A photo's name as the hub gives it: 16 hex characters and `.jpg`, `.png` or `.webp`. Nothing
/// else is ever put in a path.
pub fn photo_name_ok(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else {
        return false;
    };
    stem.len() == 16
        && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && ["jpg", "png", "webp"].contains(&ext)
}

/// Only bold, italic and links (http, https, mailto), as the editor makes them.
pub fn clean_text(text: &str) -> String {
    ammonia::Builder::default()
        .tags(["b", "strong", "i", "em", "a"].into_iter().collect())
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"))
        .clean(text.trim())
        .to_string()
}

fn title(text: &str, what: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("{what} needs a title"));
    }
    if text.chars().count() > MAX_TITLE {
        return Err(format!("{what}: a title of at most {MAX_TITLE} characters"));
    }
    Ok(text.to_owned())
}

/// What may be saved: titles trimmed and within limits, text cleaned, levels 0 or 1, photo names as
/// the hub gives them. A draft may have no steps yet.
pub fn check(guide: Guide) -> Result<Guide, String> {
    let guide_title = title(&guide.title, "The guide")?;
    if !(1..=5).contains(&guide.difficulty) {
        return Err("difficulty is 1 to 5".into());
    }
    if let Some(cover) = &guide.cover
        && !photo_name_ok(cover)
    {
        return Err("the cover is not a photo of this hub".into());
    }
    if guide.steps.len() > MAX_STEPS {
        return Err(format!("at most {MAX_STEPS} steps"));
    }
    let mut steps = Vec::with_capacity(guide.steps.len());
    for (i, step) in guide.steps.into_iter().enumerate() {
        let what = format!("Step {}", i + 1);
        let step_title = title(&step.title, &what)?;
        if step.lines.len() > MAX_LINES {
            return Err(format!("{what}: at most {MAX_LINES} lines"));
        }
        if step.photos.len() > MAX_PHOTOS {
            return Err(format!("{what}: at most {MAX_PHOTOS} photos"));
        }
        if !step.photos.iter().all(|p| photo_name_ok(p)) {
            return Err(format!("{what}: a photo that is not of this hub"));
        }
        let mut lines = Vec::with_capacity(step.lines.len());
        for line in step.lines {
            if line.level > 1 {
                return Err(format!("{what}: a line's level is 0 or 1"));
            }
            if line.text.chars().count() > MAX_TEXT {
                return Err(format!("{what}: a line of at most {MAX_TEXT} characters"));
            }
            let text = clean_text(&line.text);
            if text.is_empty() {
                continue;
            }
            lines.push(Line { text, ..line });
        }
        steps.push(Step {
            title: step_title,
            lines,
            photos: step.photos,
        });
    }
    Ok(Guide {
        title: guide_title,
        difficulty: guide.difficulty,
        cover: guide.cover,
        steps,
    })
}

impl Guide {
    /// The cover: the one chosen, else the first photo of any step.
    pub fn cover_photo(&self) -> Option<&str> {
        self.cover.as_deref().or_else(|| {
            self.steps
                .iter()
                .find_map(|s| s.photos.first())
                .map(String::as_str)
        })
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A step's lines as HTML (`<ul class="gl-lines">`). The text was cleaned by [`check`] and is
/// cleaned again here, so HTML from anywhere else cannot slip through.
pub fn lines_html(step: &Step) -> String {
    let mut out = String::from("<ul class=\"gl-lines\">");
    for line in &step.lines {
        let _ = write!(
            out,
            "<li class=\"gl-line gl-{} gl-level-{}\"><span class=\"gl-mark\" aria-hidden=\"true\"></span><span class=\"gl-text\">{}</span></li>",
            line.style.name(),
            line.level.min(1),
            clean_text(&line.text),
        );
    }
    out.push_str("</ul>");
    out
}

/// A step's photos as HTML: the first large, all as thumbnails that show it large
/// (`data-photo`). `url` gives a photo's address.
pub fn photos_html(step: &Step, url: impl Fn(&str) -> String) -> String {
    let photos: Vec<&String> = step.photos.iter().filter(|p| photo_name_ok(p)).collect();
    let Some(first) = photos.first() else {
        return String::from("<div class=\"gl-photos gl-none\"></div>");
    };
    let mut out = format!(
        "<div class=\"gl-photos\"><div class=\"gl-big\"><img data-big src=\"{}\" alt=\"\"></div>",
        escape(&url(first))
    );
    if photos.len() > 1 {
        out.push_str("<div class=\"gl-thumbs\">");
        for (i, photo) in photos.iter().enumerate() {
            let src = escape(&url(photo));
            let _ = write!(
                out,
                "<button type=\"button\" class=\"gl-thumb{}\" data-photo=\"{src}\" aria-label=\"Photo {}\"><img src=\"{src}\" alt=\"\"></button>",
                if i == 0 { " on" } else { "" },
                i + 1
            );
        }
        out.push_str("</div>");
    }
    out.push_str("</div>");
    out
}

/// The look of a guide's lines: the marking colours are fixed (the same in light and dark, so a line
/// always matches the marking in its photo); a hexagon per colour line, a triangle for caution, an
/// ⓘ for a note.
pub const STYLE: &str = r#"
.gl-lines { list-style: none; margin: 0; padding: 0; }
.gl-line { display: flex; gap: 12px; align-items: flex-start; margin: 0 0 14px; line-height: 1.5; }
.gl-level-1 { margin-left: 30px; }
.gl-mark { flex: none; width: 14px; height: 16px; margin-top: 4px; clip-path: polygon(50% 0, 100% 25%, 100% 75%, 50% 100%, 0 75%, 0 25%); background: var(--gl-c); }
.gl-black { --gl-c: #1d1d1b; } .gl-red { --gl-c: #c8102e; } .gl-orange { --gl-c: #f28c28; }
.gl-yellow { --gl-c: #f2c500; } .gl-green { --gl-c: #2bb673; } .gl-light_blue { --gl-c: #29abe2; }
.gl-blue { --gl-c: #2e5bdb; } .gl-violet { --gl-c: #9b4dca; }
:root[data-theme="dark"] .gl-black .gl-mark { box-shadow: inset 0 0 0 1.5px #8a8a85; }
.gl-caution { color: #c8102e; }
.gl-caution .gl-mark, .gl-note .gl-mark { width: 18px; height: 18px; margin-top: 2px; clip-path: none; background: none; display: grid; place-items: center; }
.gl-caution .gl-mark::before { content: ""; width: 18px; height: 16px; background: #c8102e; clip-path: polygon(50% 0, 100% 100%, 0 100%); }
.gl-note .gl-mark { box-sizing: border-box; border: 1.5px solid currentColor; border-radius: 50%; font: 700 11px/1 serif; }
.gl-note .gl-mark::before { content: "i"; }
.gl-text a { color: inherit; }
.gl-photos .gl-big { background: var(--surface, #fff); border: 1px solid var(--line, #ddd); display: grid; place-items: center; overflow: hidden; }
.gl-photos .gl-big img { max-width: 100%; max-height: 75vh; object-fit: contain; display: block; }
.gl-photos.gl-none { display: none; }
.gl-thumbs { display: flex; gap: 10px; margin-top: 10px; flex-wrap: wrap; }
.gl-thumb { padding: 0; width: 96px; aspect-ratio: 4 / 3; border: 2px solid var(--line, #ddd); background: var(--surface, #fff); cursor: pointer; }
.gl-thumb.on { border-color: var(--accent, #93a682); }
.gl-thumb img { width: 100%; height: 100%; object-fit: contain; display: block; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, style: Style, level: u8) -> Line {
        Line {
            text: text.into(),
            style,
            level,
        }
    }

    fn guide(lines: Vec<Line>, photos: Vec<String>) -> Guide {
        Guide {
            title: "  Change the tape ".into(),
            difficulty: 2,
            cover: None,
            steps: vec![Step {
                title: "Open the lid".into(),
                lines,
                photos,
            }],
        }
    }

    #[test]
    fn check_trims_and_cleans() {
        let g = check(guide(
            vec![
                line("<b>Press</b> the <script>x</script>button", Style::Green, 0),
                line(
                    "<a href=\"javascript:alert(1)\">no</a> <a href=\"https://x.org\">yes</a>",
                    Style::Note,
                    1,
                ),
                line("   ", Style::Black, 0),
            ],
            vec!["0123456789abcdef.jpg".into()],
        ))
        .unwrap();
        assert_eq!(g.title, "Change the tape");
        let lines = &g.steps[0].lines;
        assert_eq!(lines.len(), 2, "an empty line is dropped");
        assert_eq!(lines[0].text, "<b>Press</b> the button");
        assert!(!lines[1].text.contains("javascript"));
        assert!(lines[1].text.contains("href=\"https://x.org\""));
        assert_eq!(g.cover_photo(), Some("0123456789abcdef.jpg"));
    }

    #[test]
    fn check_refuses() {
        assert!(check(guide(vec![line("a", Style::Red, 2)], vec![])).is_err());
        assert!(check(guide(vec![], vec!["../etc/passwd".into()])).is_err());
        assert!(check(guide(vec![], vec!["0123456789ABCDEF.jpg".into()])).is_err());
        let mut g = guide(vec![], vec![]);
        g.difficulty = 0;
        assert!(check(g).is_err());
        let mut g = guide(vec![], vec![]);
        g.steps[0].title = " ".into();
        assert!(check(g).is_err());
    }

    #[test]
    fn styles_round_trip_by_name() {
        for style in Style::ALL {
            let json = serde_json::to_string(&style).unwrap();
            assert_eq!(json, format!("\"{}\"", style.name()));
            assert_eq!(serde_json::from_str::<Style>(&json).unwrap(), style);
        }
    }

    #[test]
    fn html_of_a_step() {
        let g = check(guide(
            vec![
                line("Mind the <i>blade</i>", Style::Caution, 0),
                line("Lift", Style::LightBlue, 1),
            ],
            vec!["0123456789abcdef.jpg".into(), "fedcba9876543210.png".into()],
        ))
        .unwrap();
        let html = lines_html(&g.steps[0]);
        assert!(html.contains("gl-caution gl-level-0"));
        assert!(html.contains("gl-light_blue gl-level-1"));
        assert!(html.contains("Mind the <i>blade</i>"));
        let photos = photos_html(&g.steps[0], |p| format!("/guides/photos/{p}"));
        assert!(photos.contains("src=\"/guides/photos/0123456789abcdef.jpg\""));
        assert_eq!(
            photos.matches("gl-thumb").count(),
            3,
            "two thumbs and their box"
        );
    }
}
