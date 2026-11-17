// Ported from CUPS 2.4.20 (reference/cups-2.4.20/cups/ppd-mark.c, ppd-page.c, ppd-emit.c, ppd.h).
// Copyright © 2020-2025 by OpenPrinting, 2007-2019 by Apple Inc. Apache-2.0 with the GPL/LGPL exception.

//! The PPD calls `raster2dymolw` makes (`CupsFilter.h:218-247`): `ppdMarkDefaults`, `ppdMarkOption`,
//! `cupsMarkOptions`, `ppdFindMarkedChoice`, on a [`PpdFile`] whose data is the plain 450's PPD
//! ([`super::ppd_lw450`], generated from `lw450.ppd`). The calls the raster interpreter makes on it
//! (`ppdPageSize`, `ppdEmitString`) are here too.

use super::options::{cups_get_option, CupsOption};

/// `ppd_choice_t`'s `choice`, `code` and the PPD line of its `*Option Choice` (ours: `line`).
#[derive(Debug, Clone, Copy)]
pub struct Choice {
    pub name: &'static str,
    /// The choice's translation string (`*PageSize w72h72/11353 Multi-Purpose:`), as the PPD has it;
    /// None when the line has none. Only for the hub's UI (HS1); never sent.
    pub text: Option<&'static str>,
    pub code: &'static str,
    pub line: u32,
}

/// `ppd_option_t` as the PPD gives it (`ppd.h`): keyword, default choice, `*OrderDependency`, choices.
#[derive(Debug, Clone, Copy)]
pub struct OptionData {
    pub keyword: &'static str,
    pub default_choice: &'static str,
    pub default_line: u32,
    pub order: f32,
    pub order_line: u32,
    pub section: &'static str,
    pub choices: &'static [Choice],
}

/// `ppd_size_t` as the PPD gives it: `*PageSize` code, `*PaperDimension`, `*ImageableArea`.
#[derive(Debug, Clone, Copy)]
pub struct SizeData {
    pub name: &'static str,
    pub page_size_line: u32,
    pub page_size: [u32; 2],
    pub cups_media_type: u32,
    pub cups_integer0: i32,
    pub imageable_area_line: u32,
    pub imageable_area: [f32; 4],
    pub paper_dimension_line: u32,
    pub paper_dimension: [f32; 2],
}

/// `ppd_size_t` (`ppd.h`) of a size: width, length, margins (`left`, `bottom`, `right`, `top`).
#[derive(Debug, Clone, Copy)]
pub struct PpdSize {
    pub name: &'static str,
    pub width: f32,
    pub length: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

/// `ppd_file_t` (the parts used): `modelname`, the options with their marks, the sizes with theirs.
pub struct PpdFile {
    pub modelname: &'static str,
    options: &'static [OptionData],
    sizes: &'static [SizeData],
    /// `choice->marked` per option (an index into its choices): the `ppd->marked` array.
    marked: Vec<Option<usize>>,
    /// `size->marked` per size.
    size_marked: Vec<bool>,
}

impl PpdFile {
    /// The plain LabelWriter 450's PPD (`lw450.ppd`), as `ppdOpenFile` would give it: nothing marked.
    pub fn lw450() -> PpdFile {
        PpdFile::new(super::ppd_lw450::MODEL_NAME, super::ppd_lw450::OPTIONS, super::ppd_lw450::SIZES)
    }

    pub fn new(modelname: &'static str, options: &'static [OptionData], sizes: &'static [SizeData]) -> PpdFile {
        PpdFile { modelname, options, sizes, marked: vec![None; options.len()], size_marked: vec![false; sizes.len()] }
    }

    /// Ours (W2): the names of the `PageSize` choices, in the PPD's order.
    pub fn page_size_names(&self) -> Vec<&'static str> {
        match self.find_option("PageSize") {
            Some(o) => self.options[o].choices.iter().map(|c| c.name).collect(),
            None => Vec::new(),
        }
    }

    /// `ppdFindOption` (`ppd-mark.c:434-479`), the by-group search: the option, compared without case.
    pub fn find_option(&self, option: &str) -> Option<usize> {
        self.options.iter().position(|o| o.keyword.eq_ignore_ascii_case(option))
    }

    /// `ppdFindChoice` (`ppd-mark.c:379-399`): the choice of an option, compared without case.
    pub fn find_choice(&self, option: usize, choice: &str) -> Option<usize> {
        let mut choice = choice;
        if choice.starts_with('{') || (choice.len() >= 7 && choice[..7].eq_ignore_ascii_case("Custom.")) {
            choice = "Custom";
        }
        self.options[option].choices.iter().position(|c| c.name.eq_ignore_ascii_case(choice))
    }

    /// `ppdFindMarkedChoice` (`ppd-mark.c:405-428`): the marked choice of an option, as its [`Choice`].
    pub fn find_marked_choice(&self, option: &str) -> Option<&'static Choice> {
        let o = self.find_option(option)?;
        self.marked[o].map(|c| &self.options[o].choices[c])
    }

    /// `ppdPageSize(ppd, name)` (`ppd-page.c:28-171`), not custom: the size named (compared without case),
    /// or, with no name, the marked one.
    pub fn page_size(&self, name: Option<&str>) -> Option<PpdSize> {
        let index = match name {
            Some(name) => {
                if name.starts_with("Custom.") {
                    // `ppd->variable_sizes` is false: the PPD has no custom size choice here
                    return None;
                }
                self.sizes.iter().position(|s| s.name.eq_ignore_ascii_case(name))?
            }
            None => self.size_marked.iter().position(|m| *m)?,
        };
        let s = &self.sizes[index];
        Some(PpdSize {
            name: s.name,
            width: s.paper_dimension[0],
            length: s.paper_dimension[1],
            left: s.imageable_area[0],
            bottom: s.imageable_area[1],
            right: s.imageable_area[2],
            top: s.imageable_area[3],
        })
    }

    /// `ppdMarkDefaults` (`ppd-mark.c:511-545`): "Clean out the marked array... Then repopulate it with the
    /// defaults". (Its last step `ppdConflicts` only flags conflicts and marks nothing: not ported.)
    pub fn mark_defaults(&mut self) {
        for m in self.marked.iter_mut() {
            *m = None;
        }

        // `ppd_defaults` (:731-748): all options of the group, "PageRegion" aside. The PPD has no groups
        // (`*OpenGroup`): its options are in one.
        for i in 0..self.options.len() {
            if !self.options[i].keyword.eq_ignore_ascii_case("PageRegion") {
                let (k, d) = (self.options[i].keyword, self.options[i].default_choice);
                self.mark_option_quick(k, d);
            }
        }
    }

    /// `ppdMarkOption` (`ppd-mark.c:554-585`): marks and counts conflicts (`ppdConflicts`, not ported, so no
    /// count is returned).
    pub fn mark_option(&mut self, option: &str, choice: &str) {
        self.mark_option_quick(option, choice);
    }

    /// `ppd_mark_option` (`ppd-mark.c:780-1141`): "Quick mark an option without checking for conflicts".
    fn mark_option_quick(&mut self, option: &str, choice: &str) {
        // :798-818 AP_D_InputSlot: the PPD has no InputSlot option (`ppdFindOption` NULL): nothing to clear
        let mut option_name = option;
        if option_name.eq_ignore_ascii_case("media") || option_name.eq_ignore_ascii_case("media-col") {
            option_name = "PageSize";
        }

        let Some(o) = self.find_option(option_name) else { return };

        let c: usize;
        let mut choice = choice;
        if choice.len() >= 7 && choice[..7].eq_ignore_ascii_case("Custom.") {
            // custom option: `ppdFindChoice(o, "Custom")`; the PPD has no "Custom" choice
            match self.find_choice(o, "Custom") {
                None => return,
                Some(_) => unreachable!("the plain 450's PPD has no Custom choice"),
            }
        } else if option_name.eq_ignore_ascii_case("PageRegion") || option_name.eq_ignore_ascii_case("PageSize") {
            // page sizes as PPD, legacy, or PWG names
            let Some(ppdsize) = self.page_size(Some(choice)) else {
                // not a PPD size name: `_ppdCacheGetPageSize(ppd->cache, ...)`, no cache here: NULL
                return;
            };
            choice = ppdsize.name;
            match self.find_choice(o, choice) {
                Some(i) => c = i,
                None => return,
            }
        } else if choice.starts_with('{') {
            // multi-value custom options
            if self.find_choice(o, "Custom").is_none() {
                return;
            }
            unreachable!("the plain 450's PPD has no Custom choice");
        } else {
            match self.options[o].choices.iter().position(|ch| ch.name.eq_ignore_ascii_case(choice)) {
                Some(i) => c = i,
                None => return,
            }
        }

        // option found; mark it and then handle unmarking any other options. (All its options are
        // PickOne: `o->ui != PPD_UI_PICKMANY`.)
        self.marked[o] = None; // unmark the old choice

        if option_name.eq_ignore_ascii_case("PageSize") || option_name.eq_ignore_ascii_case("PageRegion") {
            // mark current page size
            for j in 0..self.sizes.len() {
                self.size_marked[j] = self.sizes[j].name.eq_ignore_ascii_case(choice);
            }

            // unmark the current PageSize or PageRegion setting, as appropriate
            let other = if option_name.eq_ignore_ascii_case("PageSize") { "PageRegion" } else { "PageSize" };
            if let Some(oo) = self.find_option(other) {
                self.marked[oo] = None;
            }
        }
        // InputSlot / ManualFeed (:945-1000): the PPD has neither

        self.marked[o] = Some(c);
    }

    /// `cupsMarkOptions` (`ppd-mark.c:48-370`): "Do special handling for finishings, media, output-bin,
    /// output-mode, print-color-mode, print-quality, and PageSize", then "Mark other options". Not ported:
    /// the PPD-cache mappings of `media`, `PageSize` (given as an option), `output-bin`, `print-color-mode`,
    /// `print-quality`, `sides` and the presets (:79-290), `multiple-document-handling`, `finishings`,
    /// `APPrinterPreset`, `mirror` (:310-370): the job passes no options (W3), the tests only the PPD's own.
    /// Returns 0 for no conflicts (`ppdConflicts` not ported).
    pub fn cups_mark_options(&mut self, options: &[CupsOption]) -> i32 {
        if options.is_empty() {
            return 0;
        }

        for optptr in options {
            if optptr.name.eq_ignore_ascii_case("media")
                || optptr.name.eq_ignore_ascii_case("output-bin")
                || optptr.name.eq_ignore_ascii_case("output-mode")
                || optptr.name.eq_ignore_ascii_case("print-quality")
                || optptr.name.eq_ignore_ascii_case("sides")
            {
                continue;
            } else if optptr.name.eq_ignore_ascii_case("resolution") || optptr.name.eq_ignore_ascii_case("printer-resolution") {
                self.mark_option_quick("Resolution", &optptr.value);
                // SetResolution, JCLResolution, CNRes_PGP: options this PPD does not have
            } else if optptr.name.eq_ignore_ascii_case("multiple-document-handling")
                || optptr.name.eq_ignore_ascii_case("finishings")
                || optptr.name.eq_ignore_ascii_case("APPrinterPreset")
            {
                // see above: not ported
            } else if optptr.name.eq_ignore_ascii_case("mirror") {
                self.mark_option_quick("MirrorPrint", &optptr.value);
            } else {
                // (`set_pagesize`, `set_source`, `set_type` are false: `media` is not given)
                self.mark_option_quick(&optptr.name, &optptr.value);
            }
        }

        let _ = cups_get_option;
        0
    }

    /// `ppdEmitString(ppd, PPD_ORDER_ANY, 0.0)` (`ppd-emit.c:612-`) with `ppdCollect2` (`:80-220`): the
    /// codes of the marked choices of the `AnySetup` options, by their `*OrderDependency`. `ppd_handle_media`
    /// (:1132-1225) first: no `ManualFeed`/`InputSlot`, so `ppdMarkOption(ppd, "PageSize", size->name)`.
    /// The `[{ %%BeginFeature ... } stopped cleartomark` wrapper of each code is not written: the
    /// interpreter takes each `<<...>>setpagedevice` as it is (not covered).
    pub fn emit_codes_any(&mut self) -> Vec<&'static str> {
        // ppd_handle_media
        if let Some(size) = self.page_size(None) {
            self.mark_option("PageSize", size.name);
        }

        // ppdCollect2: the marked array is sorted by option keyword (`ppd_compare_choices`, ppd.c:2500)
        let mut idx: Vec<usize> = (0..self.options.len()).filter(|i| self.marked[*i].is_some()).collect();
        idx.sort_by(|a, b| self.options[*a].keyword.cmp(self.options[*b].keyword));

        let mut collect: Vec<(usize, f32)> = Vec::new();
        for i in idx {
            if self.options[i].section == "AnySetup" {
                collect.push((i, self.options[i].order));
            }
        }

        // "If we have more than 1 marked choice, sort them..." (:188-203)
        let count = collect.len();
        if count > 1 {
            for i in 0..count - 1 {
                for j in i + 1..count {
                    if collect[i].1 > collect[j].1 {
                        collect.swap(i, j);
                    }
                }
            }
        }

        collect
            .into_iter()
            .map(|(i, _)| self.options[i].choices[self.marked[i].unwrap()].code)
            .collect()
    }
}
