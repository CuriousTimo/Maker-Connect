// Ported from DYMO's CUPS driver 1.4.0 (reference/dymo-cups-drivers-1.4.0): the driver's own tests,
//   src/lw/tests/TestLabelWriterDriver.cpp, TestLabelWriterFilter.cpp, TestLabelWriterLanguageMonitor.cpp,
//   MOCK_LWLMPrintEnvironment.{h,cpp}, src/common/tests/MOCK_PrintEnvironment.{h,cpp},
//   TestErrorDiffusionHalftoning.cpp, TestNLLHalftoning.cpp.
// DYMO LabelWriter Drivers, Copyright (C) 2008 Sanford L.P.
// GPL-2 or any later version (taken as version 3, so it combines with this program's AGPL-3.0).

//! The driver's tests, each case, with `MOCK_PrintEnvironment` and `MOCK_LWLMPrintEnvironment`, then the
//! tests of the CUPS runtime and the top edge.
//!
//! Not covered: `LabelWriterDriverTest::testTwinTurboRoll` and `LabelWriterFilterTest::testRoll` (the Twin
//! Turbo driver is not ported). `LabelWriterFilterTest` opens `lwtt.ppd` and builds a Twin Turbo driver;
//! here `testDensity` and `testQuality` use the plain 450's PPD and `CLabelWriterDriver400` (the options and
//! the initializer are the same code).

use super::cups::options::cups_parse_options;
use super::cups::ppd::PpdFile;
use super::cups_filter_label_writer::process_ppd_options;
use super::dummy_language_monitor::DummyLanguageMonitor;
use super::error_diffusion_halftoning::ErrorDiffusionHalftoning;
use super::halftoning::{HalftoneFilter, ImageBuffer, ImageType};
use super::label_writer_driver::{
    Density, LabelWriterDriver, PaperType, Quality, Roll, get_roll_select_command,
};
use super::label_writer_language_monitor::LabelWriterLanguageMonitor;
use super::nll_halftoning::NllHalftoning;
use super::printer_driver::{Buffer, JobStatus, LanguageMonitor, PrintEnvironment, PrinterDriver};
use std::cell::RefCell;
use std::rc::Rc;

const ESC: u8 = 0x1B;
const SYN: u8 = 0x16;
const ETB: u8 = 0x17;

/// `MockPrintEnvironment` (`MOCK_PrintEnvironment.cpp`).
#[derive(Default)]
struct MockPrintEnvironment {
    data: Buffer,
}

impl MockPrintEnvironment {
    fn get_data(&self) -> Buffer {
        self.data.clone()
    }
    fn clear_data(&mut self) {
        self.data.clear();
    }
}

impl PrintEnvironment for MockPrintEnvironment {
    fn write_data(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
    }
    fn read_data(&mut self) -> Buffer {
        Buffer::new()
    }
    fn get_job_status(&mut self) -> JobStatus {
        JobStatus::Ok
    }
    fn set_job_status(&mut self, _job_status: JobStatus) {}
}

/// `MockLWLMPrintEnvironment::mode_t`.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Tof,
    PaperOut,
    NotTof,
    Failed,
    RollChanged,
}

const TOF_BIT: u8 = 0x02;
const ROLL_CHANGED_BIT: u8 = 0x08;
const PAPER_OUT_BIT: u8 = 0x20;
const ERROR_BIT: u8 = 0x80;

/// `MockLWLMPrintEnvironment` (`MOCK_LWLMPrintEnvironment.cpp`): "emulate responces from the LabelWriter".
struct MockLwlmPrintEnvironment {
    base: MockPrintEnvironment,
    mode: Vec<Mode>,
}

impl MockLwlmPrintEnvironment {
    fn new() -> Self {
        MockLwlmPrintEnvironment {
            base: MockPrintEnvironment::default(),
            mode: vec![Mode::Tof],
        }
    }
    fn set_mode(&mut self, mode: Mode) {
        self.mode.clear();
        self.mode.push(mode);
    }
    fn push_mode(&mut self, mode: Mode, count: usize) {
        for _ in 0..count {
            self.mode.push(mode);
        }
    }
    fn get_data(&self) -> Buffer {
        self.base.get_data()
    }
    fn clear_data(&mut self) {
        self.base.clear_data();
    }
}

impl PrintEnvironment for MockLwlmPrintEnvironment {
    fn write_data(&mut self, data: &[u8]) {
        // don't store request status requests
        if data != LabelWriterDriver::get_request_status_command().as_slice() {
            self.base.write_data(data);
        }
    }

    fn read_data(&mut self) -> Buffer {
        let mut data = Buffer::new();

        let mode = if self.mode.len() > 1 {
            self.mode.pop().unwrap()
        } else {
            *self.mode.last().unwrap()
        };

        match mode {
            Mode::Tof => data.push(TOF_BIT),
            Mode::PaperOut => data.push(ERROR_BIT | PAPER_OUT_BIT),
            Mode::RollChanged => data.push(TOF_BIT | ROLL_CHANGED_BIT),
            Mode::NotTof => data.push(0),
            Mode::Failed => {}
        }
        data
    }

    fn get_job_status(&mut self) -> JobStatus {
        JobStatus::Ok
    }

    fn set_job_status(&mut self, job_status: JobStatus) {
        match job_status {
            JobStatus::Ok | JobStatus::PaperOut | JobStatus::Error => {}
            _ => unreachable!("assert(0)"),
        }
    }
}

fn reset() -> Buffer {
    vec![ESC; 156]
}

fn cat(parts: &[&[u8]]) -> Buffer {
    parts.concat()
}

// ---- TestLabelWriterDriver.cpp ------------------------------------------------------------------

/// `LabelWriterDriverTest::testPrologEpilogCommand`
#[test]
fn driver_test_prolog_epilog_command() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new(env.clone());

    driver.start_doc();
    driver.start_page();
    driver.end_page();
    driver.end_doc();

    let printer_commands = cat(&[
        &reset(),
        &[
            ESC, b'Q', 0x00, 0x00, ESC, b'B', 0, ESC, b'h', ESC, b'e', ESC, b'L', 0x08, 0x00, ESC,
            b'E',
        ],
    ]);
    assert_eq!(printer_commands, env.borrow().get_data());
}

/// `LabelWriterDriverTest::testCompression`
#[test]
fn driver_test_compression() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new(env.clone());

    let line1 = [0xff; 8];
    let line2 = [0xaa; 8];
    let line3 = [0xff; 24];

    driver.process_raster_line(&line1);
    driver.process_raster_line(&line2);
    driver.process_raster_line(&line3);

    let printer_commands = [
        ESC, b'B', 0, ESC, b'D', 0x08, ETB, 0xbf, ESC, b'B', 0, SYN, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
        0xaa, 0xaa, 0xaa, ESC, b'B', 0, ESC, b'D', 0x18, ETB, 0xff, 0xbf,
    ];
    assert_eq!(printer_commands.to_vec(), env.borrow().get_data());
}

/// `LabelWriterDriverTest::testEmptyLines`
#[test]
fn driver_test_empty_lines() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new(env.clone());

    let line1 = [0xff; 8];
    let line2 = [0x00; 8];

    for _ in 0..255 {
        driver.process_raster_line(&line2);
    }
    for _ in 0..2 {
        driver.process_raster_line(&[]);
    }
    driver.process_raster_line(&line1);

    let printer_commands = [
        ESC, b'f', 1, 0xff, ESC, b'f', 1, 2, ESC, b'B', 0, ESC, b'D', 0x08, ETB, 0xbf,
    ];
    assert_eq!(printer_commands.to_vec(), env.borrow().get_data());
}

/// `LabelWriterDriverTest::testDotTab`
#[test]
fn driver_test_dot_tab() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new(env.clone());

    let line1 = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let line2 = [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let line3 = [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let line4 = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

    driver.process_raster_line(&line1);
    driver.process_raster_line(&line2);
    driver.process_raster_line(&line3);
    driver.process_raster_line(&line4);

    let printer_commands = [
        ESC, b'B', 0, ESC, b'D', 0x08, ETB, 0xbf, ESC, b'B', 2, ESC, b'D', 0x06, ETB, 0xaf, ESC,
        b'B', 2, ETB, 0xaf, ESC, b'B', 1, ESC, b'D', 0x07, ETB, 0xb7,
    ];
    assert_eq!(printer_commands.to_vec(), env.borrow().get_data());
}

/// `LabelWriterDriverTest::testContinuousPaper`
#[test]
fn driver_test_continuous_paper() {
    let env1 = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let env2 = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver1 = LabelWriterDriver::new(env1.clone());
    let mut driver2 = LabelWriterDriver::new(env2.clone());

    driver1.set_page_height(0x1234);
    driver1.set_paper_type(PaperType::Regular);
    driver1.start_page();

    driver2.set_page_height(0x1234);
    driver2.set_paper_type(PaperType::Continuous);
    driver2.start_page();

    assert_eq!(vec![ESC, b'L', 0x12, 0x34], env1.borrow().get_data());
    assert_eq!(vec![ESC, b'L', 0xff, 0xff], env2.borrow().get_data());
}

/// `LabelWriterDriverTest::test400EndPageEndDoc`
#[test]
fn driver_test_400_end_page_end_doc() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new400(env.clone());

    driver.start_doc();
    driver.start_page();
    driver.end_page();
    driver.start_page();
    driver.end_page();
    driver.end_doc();

    let printer_commands = cat(&[
        &reset(),
        &[
            ESC, b'Q', 0x00, 0x00, ESC, b'B', 0, ESC, b'h', ESC, b'e', ESC, b'L', 0x08, 0x00, ESC,
            b'G', ESC, b'L', 0x08, 0x00, ESC, b'G', ESC, b'E',
        ],
    ]);
    assert_eq!(printer_commands, env.borrow().get_data());
}

/// The base class (the 320's, `raster2dymolw.cpp:111-123`): `CLabelWriterDriver::EndPage` is the form feed
/// (`LabelWriterDriver.cpp:77-81`) and `EndDoc` sends nothing (:57-60); width 84 (:38).
#[test]
fn driver_test_base_end_page_end_doc() {
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let mut driver = LabelWriterDriver::new(env.clone());

    driver.start_doc();
    driver.start_page();
    driver.end_page();
    driver.start_page();
    driver.end_page();
    driver.end_doc();

    let printer_commands = cat(&[
        &reset(),
        &[
            ESC, b'Q', 0x00, 0x00, ESC, b'B', 0, ESC, b'h', ESC, b'e', ESC, b'L', 0x08, 0x00, ESC,
            b'E', ESC, b'L', 0x08, 0x00, ESC, b'E',
        ],
    ]);
    assert_eq!(printer_commands, env.borrow().get_data());
}

/// `raster2dymolw.cpp:78-123`: the class by the PPD's model name; the 320's is in neither list.
#[test]
fn the_320s_ppd_is_the_base_class() {
    use super::cups::ppd::PpdFile;
    assert_eq!(PpdFile::lw320().modelname, "DYMO LabelWriter 320");
    assert_eq!(PpdFile::lw450().modelname, "DYMO LabelWriter 450");
}

// ---- TestLabelWriterFilter.cpp ------------------------------------------------------------------

fn filter_driver() -> (
    Rc<RefCell<MockPrintEnvironment>>,
    LabelWriterDriver,
    PpdFile,
) {
    // setUp (:35-45): the PPD opened and its defaults marked, the driver, the monitor (a dummy one, which
    // `ProcessPPDOptions` does not use)
    let mut ppd = PpdFile::lw450();
    ppd.mark_defaults();
    let env = Rc::new(RefCell::new(MockPrintEnvironment::default()));
    let _lm = DummyLanguageMonitor::new(env.clone());
    let driver = LabelWriterDriver::new400(env.clone());
    (env, driver, ppd)
}

/// `LabelWriterFilterTest::testDensity`
#[test]
fn filter_test_density() {
    let (_env, mut driver, mut ppd) = filter_driver();
    ppd.mark_option("DymoPrintDensity", "Light");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Density::Low, driver.get_density());

    ppd.mark_option("DymoPrintDensity", "Medium");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Density::Medium, driver.get_density());

    ppd.mark_option("DymoPrintDensity", "Normal");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Density::Normal, driver.get_density());

    ppd.mark_option("DymoPrintDensity", "Dark");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Density::High, driver.get_density());
}

/// `LabelWriterFilterTest::testQuality`
#[test]
fn filter_test_quality() {
    let (_env, mut driver, mut ppd) = filter_driver();
    ppd.mark_option("DymoPrintQuality", "Text");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Quality::Text, driver.get_quality());

    ppd.mark_option("DymoPrintQuality", "Graphics");
    process_ppd_options(&mut driver, &ppd);
    assert_eq!(Quality::BarcodeAndGraphics, driver.get_quality());
}

// ---- TestLabelWriterLanguageMonitor.cpp -----------------------------------------------------------

fn lm_with(env: &Rc<RefCell<MockLwlmPrintEnvironment>>) -> LabelWriterLanguageMonitor {
    // CLabelWriterLanguageMonitor LM(Env, false, 1); the environment variable DEVICE_URI is not set in the tests
    LabelWriterLanguageMonitor::new(env.clone(), false, 1, None)
}

/// `LabelWriterLMTest::testGoodJob`
#[test]
fn lm_test_good_job() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    env.borrow_mut().set_mode(Mode::Tof);

    lm.start_doc();
    env.borrow_mut().clear_data(); // don't need reset command
    lm.start_page();
    lm.process_data(&printer_data2);

    lm.end_page();
    lm.end_doc();

    assert_eq!(Buffer::new(), env.borrow().get_data());
}

/// `LabelWriterLMTest::testPaperOut`
#[test]
fn lm_test_paper_out() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let output = vec![ESC, b'G', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    env.borrow_mut().set_mode(Mode::Tof);

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);

    env.borrow_mut().push_mode(Mode::PaperOut, 1);
    lm.end_page();
    lm.end_doc();

    assert_eq!(output, env.borrow().get_data());
}

/// `LabelWriterLMTest::testPaperOutAfterReprint`
#[test]
fn lm_test_paper_out_after_reprint() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5];
    let output = vec![ESC, b'G', 1, 2, 3, 4, 5, ESC, b'G', 1, 2, 3, 4, 5];

    env.borrow_mut().set_mode(Mode::Tof);

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);

    env.borrow_mut().push_mode(Mode::PaperOut, 1); // this will lead to second reprint
    env.borrow_mut().push_mode(Mode::Tof, 1); // this will lead to begin of first reprint
    env.borrow_mut().push_mode(Mode::PaperOut, 1); // this will lead to first reprint
    lm.end_page();
    lm.end_doc();

    assert_eq!(output, env.borrow().get_data());
}

/// `LabelWriterLMTest::testPollForPaperIn`
#[test]
fn lm_test_poll_for_paper_in() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let output = vec![ESC, b'G', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    env.borrow_mut().set_mode(Mode::Tof);

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);

    env.borrow_mut().push_mode(Mode::PaperOut, 5);
    lm.end_page();
    lm.end_doc();

    assert_eq!(output, env.borrow().get_data());
}

/// `LabelWriterLMTest::testOneGoodPageOneNot`
#[test]
fn lm_test_one_good_page_one_not() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let page1: Buffer = vec![1, 2, 3];
    let page2: Buffer = vec![4, 5, 6];
    let output = vec![ESC, b'G', 4, 5, 6];

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&page1);
    lm.end_page();

    env.borrow_mut().push_mode(Mode::PaperOut, 1);
    lm.process_data(&page2);
    lm.end_page();
    lm.end_doc();

    assert_eq!(output, env.borrow().get_data());
}

/// `LabelWriterLMTest::testRollSynchronize`
#[test]
fn lm_test_roll_synchronize() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);

    env.borrow_mut().set_mode(Mode::Tof);

    lm.set_roll(Roll::Left);
    lm.start_doc();
    lm.end_doc();

    let mut b = LabelWriterDriver::get_reset_command();
    let b2 = get_roll_select_command(Roll::Left);
    b.extend_from_slice(&b2);

    assert_eq!(b, env.borrow().get_data());
}

/// `LabelWriterLMTest::testResetPrinter`
#[test]
fn lm_test_reset_printer() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);

    lm.start_doc();
    lm.end_doc();

    assert_eq!(
        LabelWriterDriver::get_reset_command(),
        env.borrow().get_data()
    );
}

/// `LabelWriterLMTest::testStatusReadFailed`
#[test]
fn lm_test_status_read_failed() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    env.borrow_mut().set_mode(Mode::Failed);

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);
    lm.end_page();

    lm.start_page();
    lm.process_data(&printer_data2);
    lm.end_page();
    lm.end_doc();

    assert_eq!(Buffer::new(), env.borrow().get_data());
}

/// `LabelWriterLMTest::testContinuousPaper`
#[test]
fn lm_test_continuous_paper() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    lm.set_paper_type(PaperType::Continuous);
    env.borrow_mut().set_mode(Mode::NotTof);

    lm.start_doc();
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);
    lm.end_page();

    lm.start_page();
    lm.process_data(&printer_data2);
    lm.end_page();
    lm.end_doc();

    assert_eq!(Buffer::new(), env.borrow().get_data());
}

/// `LabelWriterLMTest::testRollChangedBit`
#[test]
fn lm_test_roll_changed_bit() {
    let env = Rc::new(RefCell::new(MockLwlmPrintEnvironment::new()));
    let mut lm = lm_with(&env);
    let printer_data2: Buffer = vec![1, 2, 3, 4];
    let output1 = vec![ESC, b'G', 1, 2, 3, 4];
    let output2 = vec![1, 2, 3, 4];

    lm.start_doc();
    env.borrow_mut().clear_data();

    // test without ROLL_CHANGED
    lm.start_page();
    lm.process_data(&printer_data2);
    env.borrow_mut().push_mode(Mode::PaperOut, 1);
    lm.end_page();

    assert_eq!(output1, env.borrow().get_data());

    // test with ROLL_CHANGED
    env.borrow_mut().clear_data();
    lm.start_page();
    lm.process_data(&printer_data2);
    // three will be several requests for the status, so ROLL_CHANGED_BIT should remain for a while
    env.borrow_mut().push_mode(Mode::RollChanged, 2);
    lm.end_page();
    lm.end_doc();

    assert_eq!(output2, env.borrow().get_data());
}

// ---- common/tests/TestErrorDiffusionHalftoning.cpp, TestNLLHalftoning.cpp -----------------------------

fn process_three(input_line: &[u8], expected: [u8; 3]) {
    let mut h = ErrorDiffusionHalftoning::new(ImageType::RGB, ImageType::BW, true).unwrap();
    let mut output_line = Buffer::new();
    for e in expected {
        h.process_line(input_line, &mut output_line);
        assert_eq!(vec![e], output_line);
    }
}

/// `ErrorDiffusionTest::testBlack`: 5 pixels wide, 24bits per pixel
#[test]
fn halftone_test_black() {
    process_three(&[0; 15], [0xf8, 0xf8, 0xf8]);
}

/// `ErrorDiffusionTest::testWhite`
#[test]
fn halftone_test_white() {
    process_three(&[0xff; 15], [0x00, 0x00, 0x00]);
}

/// `ErrorDiffusionTest::testRed`
#[test]
fn halftone_test_red() {
    process_three(
        &[0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0],
        [0xe8, 0xb8, 0xd0],
    );
}

/// `ErrorDiffusionTest::testGreen`
#[test]
fn halftone_test_green() {
    process_three(
        &[0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0],
        [0x50, 0x48, 0x50],
    );
}

/// `ErrorDiffusionTest::testBlue`
#[test]
fn halftone_test_blue() {
    process_three(
        &[0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff, 0, 0, 0xff],
        [0xf8, 0xf8, 0xe8],
    );
}

/// `NonLinearLaplacianTest::testBlock` (NLL, :38-84): 8x10 pixels wide, 24bits per pixel
#[test]
fn nll_test_block() {
    let w = [255u8, 255, 255];
    let b = [0u8, 0, 0];
    let white_line: Vec<u8> = std::iter::repeat(w).take(8).flatten().collect();
    let mut black_line: Vec<u8> = Vec::new();
    for x in 0..8 {
        black_line.extend_from_slice(if (2..5).contains(&x) { &b } else { &w });
    }
    let mut input_image: ImageBuffer = Vec::new();
    for i in 0..10 {
        input_image.push(if (1..8).contains(&i) {
            black_line.clone()
        } else {
            white_line.clone()
        });
    }

    let mut output_data: Vec<u8> = Vec::new();
    for row in [0x00u8, 0x38, 0x38, 0x38, 0x38, 0x38, 0x38, 0x38, 0x00, 0x00] {
        output_data.extend_from_slice(&[row, 0]);
    }

    let mut h = NllHalftoning::new(0, ImageType::RGB, ImageType::BW).unwrap();
    let mut output_image: ImageBuffer = Vec::new();
    h.process_image(&input_image, &mut output_image);
    let output: Buffer = output_image.iter().flatten().copied().collect();
    assert_eq!(output_data, output);
}

// ---- the CUPS runtime ---------------------------------------------------------------------------------

#[test]
fn cups_parse_options_as_cups() {
    // options.c:260-465: names, booleans (`no` prefix), values, quotes, braces
    let mut o = Vec::new();
    cups_parse_options(
        "DymoHalftoning=NLL noduplex landscape copies=2 title='a b' x={k=v}",
        &mut o,
    );
    let get = |n: &str| o.iter().find(|x| x.name == n).map(|x| x.value.clone());
    assert_eq!(get("DymoHalftoning").as_deref(), Some("NLL"));
    assert_eq!(get("duplex").as_deref(), Some("false"));
    assert_eq!(get("landscape").as_deref(), Some("true"));
    assert_eq!(get("copies").as_deref(), Some("2"));
    assert_eq!(get("title").as_deref(), Some("a b"));
    assert_eq!(get("x").as_deref(), Some("{k=v}"));
    let mut none = Vec::new();
    cups_parse_options("", &mut none);
    assert!(none.is_empty());
}

#[test]
fn ppd_defaults_are_marked_as_cups_marks_them() {
    let mut ppd = PpdFile::lw450();
    ppd.mark_defaults();
    assert_eq!(ppd.find_marked_choice("Resolution").unwrap().name, "300dpi");
    assert_eq!(
        ppd.find_marked_choice("DymoHalftoning").unwrap().name,
        "ErrorDiffusion"
    );
    assert_eq!(
        ppd.find_marked_choice("DymoPrintDensity").unwrap().name,
        "Normal"
    );
    assert_eq!(
        ppd.find_marked_choice("DymoPrintQuality").unwrap().name,
        "Text"
    );
    assert_eq!(ppd.find_marked_choice("PageSize").unwrap().name, "w167h288");
    // ppd_defaults leaves PageRegion alone (ppd-mark.c:731-748)
    assert!(ppd.find_marked_choice("PageRegion").is_none());
    assert_eq!(ppd.page_size(None).unwrap().name, "w167h288");
}

// ---- the top edge (ours, W1 to W10) ------------------------------------------------------------------

mod top_edge {
    use super::super::cups::usb_backend::{UsbError, UsbHost, UsbPrinterIo};
    use super::super::port::{
        LabelWriter320, LabelWriter450, NO_JOB_CONTROL, NO_QUESTION, NO_SUCH_CONTROL,
    };
    use crate::{Event, Order, Port};
    use maker_connect_protocol::{Action, JobControl, JobOrder, MachineState};
    use std::collections::VecDeque;
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A printer on the bench: what is written is kept, an `ESC A` is answered with the next status byte
    /// (the last one for ever).
    struct BenchPrinter {
        written: Mutex<Vec<u8>>,
        statuses: Mutex<VecDeque<u8>>,
        last_status: u8,
        pending: Mutex<VecDeque<u8>>,
        fail_writes: bool,
    }

    impl UsbPrinterIo for BenchPrinter {
        fn bulk_write(&self, data: &[u8]) -> Result<usize, UsbError> {
            if self.fail_writes {
                return Err(UsbError::Other(-1));
            }
            self.written.lock().unwrap().extend_from_slice(data);
            if data == [0x1b, b'A'] {
                let s = self
                    .statuses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(self.last_status);
                self.pending.lock().unwrap().push_back(s);
            }
            Ok(data.len())
        }
        fn bulk_read(&self, _max: usize, _timeout_ms: u64) -> Result<Vec<u8>, UsbError> {
            if let Some(b) = self.pending.lock().unwrap().pop_front() {
                return Ok(vec![b]);
            }
            std::thread::sleep(Duration::from_millis(20));
            Err(UsbError::Timeout)
        }
        fn has_read_endpoint(&self) -> bool {
            true
        }
        fn close(&self) {}
    }

    /// The host of USB printers: the printer is there after `absent` looks.
    struct BenchHost {
        printer: Arc<BenchPrinter>,
        absent: Mutex<usize>,
        asked: Mutex<Vec<(String, String)>>,
    }

    impl UsbHost for BenchHost {
        fn find_device(&self, manufacturer: &str, model: &str) -> Option<Arc<dyn UsbPrinterIo>> {
            self.asked
                .lock()
                .unwrap()
                .push((manufacturer.to_owned(), model.to_owned()));
            let mut a = self.absent.lock().unwrap();
            if *a > 0 {
                *a -= 1;
                return None;
            }
            Some(self.printer.clone())
        }
    }

    fn bench(statuses: &[u8], last: u8, absent: usize) -> (Arc<BenchHost>, Arc<BenchPrinter>) {
        let printer = Arc::new(BenchPrinter {
            written: Mutex::new(Vec::new()),
            statuses: Mutex::new(statuses.iter().copied().collect()),
            last_status: last,
            pending: Mutex::new(VecDeque::new()),
            fail_writes: false,
        });
        (
            Arc::new(BenchHost {
                printer: printer.clone(),
                absent: Mutex::new(absent),
                asked: Mutex::new(Vec::new()),
            }),
            printer,
        )
    }

    /// A PNG of the given size, RGB, a black frame on white.
    fn png(name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dymo-lw450-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("label.png");
        let mut data = vec![255u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                if x < 5 || y < 5 || x >= w - 5 || y >= h - 5 {
                    let i = ((y * w + x) * 3) as usize;
                    data[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        let mut enc = png::Encoder::new(
            std::io::BufWriter::new(std::fs::File::create(&path).unwrap()),
            w,
            h,
        );
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&data).unwrap();
        path
    }

    fn job(id: i64, length: Option<u32>) -> JobOrder {
        JobOrder {
            id,
            name: "label".to_owned(),
            download: String::new(),
            size: 0,
            from_line: None,
            tape_mm: None,
            label_length_mm: length,
            page_size: None,
        }
    }

    fn start(
        host: Arc<BenchHost>,
    ) -> (
        std::sync::mpsc::Sender<Order>,
        Receiver<Event>,
        std::thread::JoinHandle<()>,
    ) {
        let (otx, orx) = channel();
        let (etx, erx) = channel();
        let port = Box::new(LabelWriter450::with_host(host));
        let h = std::thread::spawn(move || port.run(orx, etx));
        (otx, erx, h)
    }

    fn start_320(
        host: Arc<BenchHost>,
    ) -> (
        std::sync::mpsc::Sender<Order>,
        Receiver<Event>,
        std::thread::JoinHandle<()>,
    ) {
        let (otx, orx) = channel();
        let (etx, erx) = channel();
        let port = Box::new(LabelWriter320::with_host(host));
        let h = std::thread::spawn(move || port.run(orx, etx));
        (otx, erx, h)
    }

    fn until_job_end(erx: &Receiver<Event>) -> (maker_connect_protocol::JobEnd, Vec<Event>) {
        let mut seen = Vec::new();
        loop {
            match erx
                .recv_timeout(Duration::from_secs(120))
                .expect("no JobEnd")
            {
                Event::JobEnd(e) => return (e, seen),
                other => seen.push(other),
            }
        }
    }

    /// W1, W2: a job prints the label as the filter writes it; Idle, then Busy, then Idle again; nothing is
    /// sent before the job.
    #[test]
    fn a_job_prints_as_the_filter_writes_it() {
        let (host, printer) = bench(&[], 0x02, 0);
        let (otx, erx, h) = start(host.clone());
        // W5: nothing is sent while idle: the host is not even asked
        std::thread::sleep(Duration::from_millis(200));
        assert!(printer.written.lock().unwrap().is_empty());
        assert!(host.asked.lock().unwrap().is_empty());

        // w72h72: 1 in x 1 in, 271 x 254 dots at 300 dpi, 25 mm (`PaperDimension` 72 pt)
        let file = png("job", 271, 254);
        otx.send(Order::Job {
            order: job(7, Some(25)),
            file,
        })
        .unwrap();
        let (end, seen) = until_job_end(&erx);
        assert!(end.ok, "{end:?}");
        assert_eq!(end.id, 7);
        assert!(matches!(&seen[0], Event::Telemetry(t) if t.state == MachineState::Idle));
        assert!(
            seen.iter()
                .any(|e| matches!(e, Event::Telemetry(t) if t.state == MachineState::Busy))
        );
        assert_eq!(
            host.asked.lock().unwrap()[0],
            ("DYMO".to_owned(), "LabelWriter 450".to_owned())
        );

        let w = printer.written.lock().unwrap().clone();
        // the monitor's reset, the driver's reset, `ESC Q 0 0`, `ESC B 0`, quality text, density normal
        assert_eq!(&w[..156], &[0x1b; 156][..]);
        assert_eq!(&w[156..312], &[0x1b; 156][..]);
        assert_eq!(
            &w[312..326],
            &[
                0x1b, b'Q', 0, 0, 0x1b, b'B', 0, 0x1b, b'h', 0x1b, b'e', 0x1b, b'A', 0x1b
            ][..]
        );
        assert!(
            w.windows(4).any(|x| x == [0x1b, b'L', 0x01, 0x2c]),
            "ESC L 300: 72 pt at 300 dpi, `PageSize[1] * HWResolution[1] / 72`"
        );
        assert!(w.ends_with(&[0x1b, b'E']));
        // W5: idle again after it
        let last = erx.recv_timeout(Duration::from_secs(5));
        let _ = last;
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// The 320: the printer is found by `DYMO` / `LabelWriter 320`, the filter takes `CLabelWriterDriver` by the
    /// PPD's model name: the page ends with the form feed `ESC E` and the document with nothing, where the
    /// 450's page ends with `ESC G` and its document with `ESC E`.
    #[test]
    fn a_job_on_the_320_prints_with_the_base_class() {
        let (host, printer) = bench(&[], 0x02, 0);
        let (otx, erx, h) = start_320(host.clone());
        let file = png("job320", 271, 254);
        otx.send(Order::Job {
            order: job(7, Some(25)),
            file,
        })
        .unwrap();
        let (end, _seen) = until_job_end(&erx);
        assert!(end.ok, "{end:?}");
        assert_eq!(
            host.asked.lock().unwrap()[0],
            ("DYMO".to_owned(), "LabelWriter 320".to_owned())
        );
        let w = printer.written.lock().unwrap().clone();
        // the page's form feed, then the monitor's status requests (`ESC A`, as many as it makes); no
        // `ESC G` before the form feed
        let mut end = w.len();
        while w[..end].ends_with(&[0x1b, b'A']) {
            end -= 2;
        }
        assert!(w[..end].ends_with(&[0x1b, b'E']));
        assert!(!w[..end].ends_with(&[0x1b, b'G', 0x1b, b'E']));
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// W2: a picture that is not the label's size fails the job, and nothing is sent.
    #[test]
    fn a_picture_that_does_not_fit_fails() {
        let (host, printer) = bench(&[], 0x02, 0);
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(1, Some(25)),
            file: png("wrong", 100, 100),
        })
        .unwrap();
        let (end, _) = until_job_end(&erx);
        assert!(!end.ok && !end.stopped);
        assert!(
            end.reason
                .unwrap()
                .contains("no label of the PPD is 25 mm long and 100 x 100 dots")
        );
        assert!(printer.written.lock().unwrap().is_empty());
        otx.send(Order::Close).unwrap();
        h.join().unwrap();

        let (host, _) = bench(&[], 0x02, 0);
        let (otx, erx, h) = start(host);
        // no length: the PPD's default `w167h288`, 665 x 1112
        otx.send(Order::Job {
            order: job(2, None),
            file: png("default", 271, 254),
        })
        .unwrap();
        let (end, _) = until_job_end(&erx);
        assert!(
            end.reason
                .unwrap()
                .contains("the label (w167h288) needs 665 x 1112 dots")
        );
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// W6: a command, a pause, a resume, a stop and an answer outside a print fail loudly.
    #[test]
    fn the_driver_has_no_controls() {
        let (host, _) = bench(&[], 0x02, 0);
        let (otx, erx, h) = start(host);
        otx.send(Order::Command {
            id: 3,
            action: Action::Home { axes: Vec::new() },
        })
        .unwrap();
        otx.send(Order::Control(JobControl::Pause)).unwrap();
        otx.send(Order::Answer(1)).unwrap();
        let mut done = None;
        let mut problems = Vec::new();
        while problems.len() < 2 || done.is_none() {
            match erx.recv_timeout(Duration::from_secs(60)).unwrap() {
                Event::Done(d) => done = Some(d),
                Event::Telemetry(t) => {
                    if let Some(p) = t.problem {
                        problems.push(p)
                    }
                }
                _ => {}
            }
        }
        let d = done.unwrap();
        assert!(!d.ok && d.id == 3 && d.reason.as_deref() == Some(NO_SUCH_CONTROL));
        assert_eq!(
            problems,
            vec![NO_JOB_CONTROL.to_owned(), NO_QUESTION.to_owned()]
        );
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// K18 / HS4 (ours): a command that came during a print is answered; a command that came after the Close
    /// fails, "the connection closed"; none is left unanswered.
    #[test]
    fn commands_waiting_at_close_fail() {
        let (host, _) = bench(&[], 0x02, 1000);
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(4, Some(25)),
            file: png("close", 271, 254),
        })
        .unwrap();
        otx.send(Order::Command {
            id: 3,
            action: Action::Home { axes: Vec::new() },
        })
        .unwrap();
        otx.send(Order::Close).unwrap();
        let _ = otx.send(Order::Command {
            id: 5,
            action: Action::Home { axes: Vec::new() },
        });
        h.join().unwrap();
        let done: Vec<_> = erx
            .try_iter()
            .filter_map(|e| {
                if let Event::Done(d) = e {
                    Some((d.id, d.ok, d.reason))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            done,
            vec![
                (3, false, Some(NO_SUCH_CONTROL.to_owned())),
                (5, false, Some(crate::CONNECTION_CLOSED.to_owned())),
            ]
        );
    }

    /// W6, W7: the printer reports the labels out; the monitor polls; the job is stopped (what CUPS does on
    /// cancel: the filter ends). The problem is the PPD's own text.
    #[test]
    fn a_stop_ends_a_print_that_waits_for_labels() {
        // status 0xa0: error and paper out, for ever
        let (host, _printer) = bench(&[], 0xa0, 0);
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(5, Some(25)),
            file: png("stop", 271, 254),
        })
        .unwrap();
        // wait for the problem to show
        let mut problem = None;
        let mut raw = None;
        let mut keyword = None;
        for _ in 0..200 {
            if let Event::Telemetry(t) = erx.recv_timeout(Duration::from_secs(60)).unwrap() {
                if t.problem.is_some() {
                    problem = t.problem;
                    raw = t.raw_status;
                    keyword = t.state_reason;
                    break;
                }
            }
        }
        assert_eq!(problem.as_deref(), Some("Out of labels."));
        assert_eq!(raw.as_deref(), Some("a0"));
        assert_eq!(keyword.as_deref(), Some("com.dymo.out-of-paper-error"));
        // the status reads while it waits keep the keyword (the next `STATE:` changes it)
        for _ in 0..5 {
            if let Event::Telemetry(t) = erx.recv_timeout(Duration::from_secs(60)).unwrap() {
                assert_eq!(
                    t.state_reason.as_deref(),
                    Some("com.dymo.out-of-paper-error")
                );
            }
        }
        otx.send(Order::Control(JobControl::Stop)).unwrap();
        let (end, _) = until_job_end(&erx);
        assert!(!end.ok && end.stopped);
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// W8: a job finds no printer: `Gone`, and the backend's wait; when the printer is there, `Back`.
    #[test]
    fn gone_and_back_at_print_time() {
        let (host, printer) = bench(&[], 0x02, 1);
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(9, Some(25)),
            file: png("gone", 271, 254),
        })
        .unwrap();
        let (end, seen) = until_job_end(&erx);
        assert!(end.ok, "{end:?}");
        let gone = seen
            .iter()
            .position(|e| matches!(e, Event::Gone))
            .expect("Gone");
        let back = seen
            .iter()
            .position(|e| matches!(e, Event::Back))
            .expect("Back");
        assert!(gone < back);
        assert!(!printer.written.lock().unwrap().is_empty());
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// W6: a stop while the backend still waits for the printer ends the job; the wait is `sleep(5)`.
    #[test]
    fn a_stop_ends_the_wait_for_the_printer() {
        let (host, _) = bench(&[], 0x02, 1000);
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(4, Some(25)),
            file: png("wait", 271, 254),
        })
        .unwrap();
        loop {
            if matches!(
                erx.recv_timeout(Duration::from_secs(60)).unwrap(),
                Event::Gone
            ) {
                break;
            }
        }
        otx.send(Order::Control(JobControl::Stop)).unwrap();
        let (end, _) = until_job_end(&erx);
        assert!(end.stopped);
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }

    /// W1: the backend cannot write: the job fails with the backend's own text.
    #[test]
    fn a_write_error_fails_the_job_with_the_backends_text() {
        let (host, printer) = bench(&[], 0x02, 0);
        // a printer that fails every write
        let failing = Arc::new(BenchPrinter {
            written: Mutex::new(Vec::new()),
            statuses: Mutex::new(VecDeque::new()),
            last_status: 0x02,
            pending: Mutex::new(VecDeque::new()),
            fail_writes: true,
        });
        let _ = (host, printer);
        let host = Arc::new(BenchHost {
            printer: failing,
            absent: Mutex::new(0),
            asked: Mutex::new(Vec::new()),
        });
        let (otx, erx, h) = start(host);
        otx.send(Order::Job {
            order: job(6, Some(25)),
            file: png("fail", 271, 254),
        })
        .unwrap();
        let (end, _) = until_job_end(&erx);
        assert!(!end.ok && !end.stopped);
        assert_eq!(
            end.reason.as_deref(),
            Some("Unable to send data to printer.")
        );
        otx.send(Order::Close).unwrap();
        h.join().unwrap();
    }
}

/// W11: the port reads the telemetry's status byte from the driver's own `ReadData` debug line
/// (`CupsPrintEnvironment.cpp:149`); this fails if that text changes.
#[test]
fn the_read_data_debug_line_is_the_drivers_text() {
    use super::cups::ppd::PpdFile;
    use super::cups::usb_backend::BackChannel;
    use super::cups_print_environment::{CupsPrintEnvironmentForLm, FilterIo, SharedIo};
    use super::stderr;

    struct Io(std::sync::Arc<BackChannel>);
    impl FilterIo for Io {
        fn write_stdout(&mut self, _d: &[u8]) -> Result<(), i32> {
            Ok(())
        }
        fn back_channel_read(&mut self, b: &mut [u8], t: f64) -> isize {
            self.0.read(b, t)
        }
        fn getenv(&self, _n: &str) -> Option<String> {
            None
        }
        fn ppd_open_file(&self) -> Option<PpdFile> {
            None
        }
        fn cancelled(&mut self) -> bool {
            false
        }
    }
    let bc = BackChannel::new();
    bc.write(&[0xa0]);
    let io: SharedIo = Rc::new(RefCell::new(Io(bc)));
    let mut env = CupsPrintEnvironmentForLm::new(io);
    stderr::start(None);
    assert_eq!(env.read_data(), vec![0xa0]);
    let lines = stderr::take();
    assert_eq!(
        lines,
        vec!["DEBUG: CCupsPrintEnvironmentForLM::ReadData() has read 1 bytes a0\n".to_owned()]
    );
}
