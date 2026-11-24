//! The ports' geometry for the hub (HS1, `docs/archive/briefs/hs1-lw450-geometry.md`; B8 in
//! `docs/archive/plans/agent-engine.md`: "the hub takes each printer's geometry from its port's ported constants").
//! Built without the `devices` feature, so the hub can depend on it without the device crates.
//! Ours, thin: every value comes from the ported code beside it, nothing is computed here on its own.

/// One `PageSize` choice of a LabelWriter's PPD.
#[derive(Debug, Clone, PartialEq)]
pub struct PageGeometry {
    /// The choice's name (`w72h72`): unique in the PPD.
    pub name: &'static str,
    /// Its translation string as the PPD has it (`11353 Multi-Purpose`).
    pub text: Option<&'static str>,
    /// `*PaperDimension` (points): width, length.
    pub paper_dimension: [f32; 2],
    /// `*ImageableArea` (points): left, bottom, right, top.
    pub imageable_area: [f32; 4],
    /// `cupsMediaType` in the choice's code (1 = a continuous roll).
    pub cups_media_type: u32,
    /// The raster in dots, as `cups_raster_interpret_ppd` makes it for this choice (the page the
    /// port's PNG must be): `cupsWidth`, `cupsHeight`.
    pub dots: [u32; 2],
    /// The length a job gives for this choice (W2, [`lw450::length_mm`]).
    pub length_mm: u32,
    /// The `PaperDimension` width in mm, rounded as W2 rounds the length: for the hub's UI only
    /// (a job never sends it).
    pub width_mm: u32,
}

/// A roll with its labels in the page's raster: from DYMO Connect's catalogue (the 450, HS2b) or one
/// `PageSize` choice of the PPD (the 320, which DYMO Connect does not list).
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogRoll {
    pub roll: crate::ports::dymo_connect_lw450::Roll,
    /// Its PPD page, as `page_sizes` gives it.
    pub page: PageGeometry,
    /// Its labels as x, y, width, height in the raster's dots. The 450's: DYMO's twips (1/1440 in) in
    /// points (/20), less the `ImageableArea`'s left and top edge, at 300 dpi. The twips' origin is taken as
    /// the page's left edge and its top (the raster's first row): NOT YET VERIFIED on the printer.
    pub cells: Vec<[f32; 4]>,
}

/// Every `PageSize` choice of a PPD, in PPD order, marked as the port marks it for a job of that size
/// (`page.rs` `make_page`: the PPD's defaults, no options, then `PageSize`).
fn page_sizes_of(
    open: fn() -> PpdFile,
    options: &'static [OptionData],
    sizes: &'static [SizeData],
) -> Vec<PageGeometry> {
    let names = {
        let mut ppd = open();
        ppd.mark_defaults();
        ppd.page_size_names()
    };
    let choices = options
        .iter()
        .find(|o| o.keyword == "PageSize")
        .map(|o| o.choices)
        .unwrap_or(&[]);
    let opened = || Some(open());
    names
        .into_iter()
        .filter_map(|name| {
            let mut ppd = lw450::marked_ppd(&opened, &[]).ok()?;
            ppd.mark_option("PageSize", name);
            let size = ppd.page_size(None)?;
            let header = cups_raster_interpret_ppd(&mut ppd, &[]).ok()?;
            let data = sizes.iter().find(|s| s.name == name)?;
            Some(PageGeometry {
                name,
                text: choices.iter().find(|c| c.name == name).and_then(|c| c.text),
                paper_dimension: data.paper_dimension,
                imageable_area: data.imageable_area,
                cups_media_type: data.cups_media_type,
                dots: [header.cups_width, header.cups_height],
                length_mm: lw450::length_mm(size.length),
                width_mm: lw450::length_mm(size.width),
            })
        })
        .collect()
}

use crate::ports::dymo_cups::cups::ppd::{OptionData, PpdFile, SizeData};
use crate::ports::dymo_cups::cups::raster::cups_raster_interpret_ppd;

/// The DYMO LabelWriter 320 (#25): its `PageSize` choices as the port's PPD code gives them (`lw320.ppd`).
/// DYMO Connect's catalogue does not list the 320, so each choice is a roll: its text is the PPD's
/// (`11353 Multi-Purpose`, the number on DYMO's box first), its one label the whole page (ours, for the
/// hub's UI only: the PPD names no labels on a page).
pub mod lw320 {
    use super::{CatalogRoll, PageGeometry};
    use crate::ports::dymo_connect_lw450::Roll;
    use crate::ports::dymo_cups::cups::ppd::PpdFile;
    use crate::ports::dymo_cups::cups::ppd_lw320::{OPTIONS, SIZES};

    /// Every `PageSize` choice of `lw320.ppd`, in PPD order.
    pub fn page_sizes() -> Vec<PageGeometry> {
        super::page_sizes_of(PpdFile::lw320, OPTIONS, SIZES)
    }

    /// Every choice as a roll, in PPD order; its id is the choice's name.
    pub fn rolls() -> Vec<CatalogRoll> {
        page_sizes()
            .into_iter()
            .map(|page| CatalogRoll {
                roll: Roll {
                    sku: page.name,
                    name: page.text,
                    paper: page.text.unwrap_or(page.name),
                    page_size: page.name,
                    search: "",
                    label: "",
                    cells: &[],
                },
                cells: vec![[0.0, 0.0, page.dots[0] as f32, page.dots[1] as f32]],
                page,
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_page_size_of_the_ppd_is_a_roll() {
            let rolls = rolls();
            assert_eq!(rolls.len(), SIZES.len());
            let names: Vec<_> = rolls.iter().map(|r| r.page.name).collect();
            let ppd: Vec<_> = SIZES.iter().map(|s| s.name).collect();
            assert_eq!(names, ppd);
            let r = rolls.iter().find(|r| r.roll.sku == "w72h72").unwrap();
            assert_eq!(r.roll.name, Some("11353 Multi-Purpose"));
            assert_eq!(
                r.cells,
                [[0.0, 0.0, r.page.dots[0] as f32, r.page.dots[1] as f32]]
            );
        }

        /// The port's own check: a picture of the facade's dots and length is the page `make_page` takes.
        #[cfg(feature = "devices")]
        #[test]
        fn the_port_takes_every_size_the_facade_gives() {
            use crate::ports::dymo_cups::page::{Picture, make_page};
            let open = || Some(PpdFile::lw320());
            for g in page_sizes() {
                let (w, h) = (g.dots[0], g.dots[1]);
                let picture = Picture {
                    width: w,
                    height: h,
                    rgb: vec![255; (w * h * 3) as usize],
                };
                let page = make_page(&picture, Some(g.name), Some(g.length_mm), "", &open)
                    .unwrap_or_else(|e| panic!("{}: {e}", g.name));
                assert_eq!(
                    [page.header.cups_width, page.header.cups_height],
                    g.dots,
                    "{}",
                    g.name
                );
            }
        }
    }
}

/// DYMO Connect's catalogue (HS2b) on a PPD's pages: each roll on the page of its `PageSize` choice, its labels
/// in that page's raster. DYMO Connect gives the 450 and the 400 the same paper group (`Papers.xml`,
/// `PaperGroupId="LabelWriter400"`), each with its own `ExcludedPapers`; the 400's are taken out in [`lw400::rolls`].
fn catalogue_rolls(pages: &[PageGeometry]) -> Vec<CatalogRoll> {
    crate::ports::dymo_connect_lw450::ROLLS
        .iter()
        .filter_map(|roll| {
            let page = pages.iter().find(|g| g.name == roll.page_size)?.clone();
            let [left, _bottom, _right, top] = page.imageable_area;
            let top_margin = page.paper_dimension[1] - top;
            let dot = |pt: f32| pt * 300.0 / 72.0;
            let cells = roll
                .cells
                .iter()
                .map(|[x, y, w, h]| {
                    [
                        dot(x / 20.0 - left),
                        dot(y / 20.0 - top_margin),
                        dot(w / 20.0),
                        dot(h / 20.0),
                    ]
                })
                .collect();
            Some(CatalogRoll {
                roll: *roll,
                page,
                cells,
            })
        })
        .collect()
}

/// The DYMO LabelWriter 400 (#230): its `PageSize` choices as the port's PPD code gives them (`lw400.ppd`), and
/// DYMO Connect's catalogue on them, as the 450's.
pub mod lw400 {
    use super::{CatalogRoll, PageGeometry};
    use crate::ports::dymo_cups::cups::ppd::PpdFile;
    use crate::ports::dymo_cups::cups::ppd_lw400::{OPTIONS, SIZES};

    /// Every `PageSize` choice of `lw400.ppd`, in PPD order.
    pub fn page_sizes() -> Vec<PageGeometry> {
        super::page_sizes_of(PpdFile::lw400, OPTIONS, SIZES)
    }

    /// Every roll of the catalogue on the 400's pages, in its order, less the papers DYMO Connect excludes for
    /// the 400 ([`crate::ports::dymo_connect_lw400::EXCLUDED_PAPERS`]).
    pub fn rolls() -> Vec<CatalogRoll> {
        let excluded = crate::ports::dymo_connect_lw400::EXCLUDED_PAPERS;
        super::catalogue_rolls(&page_sizes())
            .into_iter()
            .filter(|r| !excluded.contains(&r.roll.paper))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_catalogue_roll_is_on_a_page_of_the_400() {
            assert_eq!(page_sizes().len(), SIZES.len());
            let rolls = rolls();
            let excluded = crate::ports::dymo_connect_lw400::EXCLUDED_PAPERS;
            let all = crate::ports::dymo_connect_lw450::ROLLS;
            let kept = all.iter().filter(|r| !excluded.contains(&r.paper)).count();
            assert_eq!(rolls.len(), kept);
            assert!(rolls.iter().all(|r| !excluded.contains(&r.roll.paper)));
            // the one page the two PPDs give apart (30321 Large Address): the 400's own
            let r = rolls.iter().find(|r| r.page.name == "w102h252").unwrap();
            // lw400.ppd: "4.32 4.32 98.40 234.96"; lw450.ppd: "4.32 3.84 98.40 234.48"
            assert_eq!(r.page.imageable_area, [4.32, 4.32, 98.40, 234.96]);
        }

        /// The port's own check: a picture of the facade's dots, by its name, is the page `make_page` takes.
        #[cfg(feature = "devices")]
        #[test]
        fn the_port_takes_every_size_the_facade_gives() {
            use crate::ports::dymo_cups::page::{Picture, make_page};
            let open = || Some(PpdFile::lw400());
            for g in page_sizes() {
                let (w, h) = (g.dots[0], g.dots[1]);
                let picture = Picture {
                    width: w,
                    height: h,
                    rgb: vec![255; (w * h * 3) as usize],
                };
                let page = make_page(&picture, Some(g.name), Some(g.length_mm), "", &open)
                    .unwrap_or_else(|e| panic!("{}: {e}", g.name));
                assert_eq!(
                    [page.header.cups_width, page.header.cups_height],
                    g.dots,
                    "{}",
                    g.name
                );
            }
        }
    }
}

/// The DYMO LabelWriter 450: its `PageSize` choices as the port's PPD code gives them.
pub mod lw450 {
    pub use super::{CatalogRoll, PageGeometry};
    use crate::ports::dymo_cups::cups::options::CupsOption;
    use crate::ports::dymo_cups::cups::ppd::PpdFile;
    use crate::ports::dymo_cups::cups::ppd_lw450::{OPTIONS, SIZES};

    /// The job's label length in mm of a `PageSize` choice: its `PaperDimension` length, in points, to mm,
    /// rounded (ours, W2). Moved here from `page.rs` (HS1); the port uses it from here.
    pub fn length_mm(length_pt: f32) -> u32 {
        (length_pt as f64 * 25.4 / 72.0).round() as u32
    }

    /// The PPD marked as `ppdMarkDefaults` and the options give it (`CupsFilter.h:225-238`). Moved here
    /// from `page.rs` (HS1); the port uses it from here.
    pub fn marked_ppd(
        open: &dyn Fn() -> Option<PpdFile>,
        options: &[CupsOption],
    ) -> Result<PpdFile, String> {
        let mut ppd = open().ok_or("no PPD")?;
        ppd.mark_defaults();
        for o in options {
            ppd.mark_option(&o.name, &o.value);
        }
        ppd.cups_mark_options(options);
        Ok(ppd)
    }

    /// Every `PageSize` choice, in PPD order, marked as the port marks it for a job of that size.
    pub fn page_sizes() -> Vec<PageGeometry> {
        super::page_sizes_of(PpdFile::lw450, OPTIONS, SIZES)
    }

    /// Every roll of the catalogue, in its order.
    pub fn rolls() -> Vec<CatalogRoll> {
        super::catalogue_rolls(&page_sizes())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_catalogue_roll_has_its_ppd_page_and_labels() {
            let rolls = rolls();
            assert_eq!(rolls.len(), crate::ports::dymo_connect_lw450::ROLLS.len());
            assert!(rolls.iter().all(|r| !r.cells.is_empty()));
            let r = rolls.iter().find(|r| r.roll.sku == "S0722530").unwrap();
            assert_eq!(r.page.name, "w72h72.2");
            assert_eq!(r.cells.len(), 2, "two labels side by side");
            assert_eq!(r.cells[0][2], r.cells[1][2]);
        }

        #[test]
        fn every_page_size_of_the_ppd_is_there() {
            let sizes = page_sizes();
            assert_eq!(sizes.len(), SIZES.len());
            let names: Vec<_> = sizes.iter().map(|s| s.name).collect();
            let ppd: Vec<_> = SIZES.iter().map(|s| s.name).collect();
            assert_eq!(names, ppd);
        }

        /// The port's own check: a picture of the facade's dots and length is the page `make_page` takes.
        #[cfg(feature = "devices")]
        #[test]
        fn the_port_takes_every_size_the_facade_gives() {
            use crate::ports::dymo_cups::page::{Picture, make_page};
            let open = || Some(PpdFile::lw450());
            for g in page_sizes() {
                let (w, h) = (g.dots[0], g.dots[1]);
                let picture = Picture {
                    width: w,
                    height: h,
                    rgb: vec![255; (w * h * 3) as usize],
                };
                match make_page(&picture, None, Some(g.length_mm), "", &open) {
                    Ok(page) => assert_eq!(
                        [page.header.cups_width, page.header.cups_height],
                        g.dots,
                        "{}",
                        g.name
                    ),
                    // a length shared by different pages of the same dots: the port refuses it as ambiguous
                    Err(e) => assert!(e.starts_with("several labels"), "{}: {e}", g.name),
                }
            }
        }
    }
}

/// W2b (proxy, 2026-11-17): a job that names its PageSize choice gets exactly that page.
#[cfg(all(test, feature = "devices"))]
mod named_page_tests {
    use super::lw450::page_sizes;
    use crate::ports::dymo_cups::cups::ppd::PpdFile;
    use crate::ports::dymo_cups::page::{Picture, make_page};

    fn blank(w: u32, h: u32) -> Picture {
        Picture {
            width: w,
            height: h,
            rgb: vec![255; (w * h * 3) as usize],
        }
    }

    #[test]
    fn the_three_address_rolls_by_name() {
        let open = || Some(PpdFile::lw450());
        for name in ["w79h252", "w79h252.1", "w79h252.2"] {
            let g = page_sizes().into_iter().find(|g| g.name == name).unwrap();
            // by length the port cannot tell them apart in this PPD
            let by_length = make_page(
                &blank(g.dots[0], g.dots[1]),
                None,
                Some(g.length_mm),
                "",
                &open,
            );
            assert!(
                by_length.unwrap_err().starts_with("several labels"),
                "{name}"
            );
            let page = make_page(
                &blank(g.dots[0], g.dots[1]),
                Some(name),
                Some(g.length_mm),
                "",
                &open,
            )
            .unwrap();
            assert_eq!(page.header.cups_page_size_name, name);
            assert_eq!([page.header.cups_width, page.header.cups_height], g.dots);
        }
    }

    #[test]
    fn an_unknown_name_or_a_wrong_picture_fails() {
        let open = || Some(PpdFile::lw450());
        let err = make_page(&blank(271, 254), Some("w1h1"), None, "", &open).unwrap_err();
        assert_eq!(err, "no PageSize w1h1 in the PPD");
        // the name is not overridden by the length, and the picture must be its raster
        assert!(make_page(&blank(272, 254), Some("w72h72"), Some(25), "", &open).is_err());
    }

    #[test]
    fn a_job_without_a_name_keeps_the_length_rule() {
        let open = || Some(PpdFile::lw450());
        let page = make_page(&blank(271, 254), None, Some(25), "", &open).unwrap();
        assert_eq!(
            [page.header.cups_width, page.header.cups_height],
            [271, 254]
        );
    }
}
