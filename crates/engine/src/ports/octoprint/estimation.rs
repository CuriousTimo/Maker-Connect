//! Port of OctoPrint 1.11.8 `src/octoprint/printer/estimation.py` (`PrintTimeEstimator`,
//! `TimeEstimationHelper`), AGPL-3.0, Copyright (C) 2014 The OctoPrint Project.
//!
//! Python `float` is `f64`; `collections.deque([], n)` is a `VecDeque` bounded by hand (`append` drops
//! the oldest entry when full); `//` on floats is [`py_floordiv`]; `sum()` over floats is
//! [`py_sum`], CPython 3.12+'s compensated sum (the reference runs on Python 3.13); `not x` on a
//! float is `x == 0.0`.
//!
//! Differences (all OURS):
//!  1. `settings().getFloat(..)` returning `None` would make Python raise `TypeError` later; it is a
//!     panic here (the defaults always hold these keys).
//!  2. Python's `ZeroDivisionError` in `estimate` (`cleanedPrintTime / totalPrintTime`) is
//!     `Err(ZeroDivisionError)`; `standard.py:1186` catches every exception from `estimate`.
//!  3. `statisticalTotalPrintTimeType` of `None` with a statistical total present and a stable
//!     estimate would make Python raise `TypeError` at `"mixed-" + None`; it is a panic here.

use super::settings::settings;
use std::collections::VecDeque;

/// `ZeroDivisionError: float division by zero`
#[derive(Debug, Clone, PartialEq)]
pub struct ZeroDivisionError;

fn get_float(path: &[&str]) -> f64 {
    settings()
        .get_float(path)
        .unwrap_or_else(|| panic!("TypeError: setting {path:?} is not a number"))
}

/// Python's `float // float` (`floatobject.c float_floor_div` through `float_divmod`)
pub fn py_floordiv(vx: f64, wx: f64) -> f64 {
    if wx == 0.0 {
        panic!("ZeroDivisionError: float floor division by zero");
    }
    let mut modulus = vx % wx;
    let mut div = (vx - modulus) / wx;
    if modulus != 0.0 {
        if (wx < 0.0) != (modulus < 0.0) {
            modulus += wx;
            div -= 1.0;
        }
    } else {
        modulus = 0.0_f64.copysign(wx);
    }
    let _ = modulus;
    if div != 0.0 {
        let mut floordiv = div.floor();
        if div - floordiv > 0.5 {
            floordiv += 1.0;
        }
        floordiv
    } else {
        0.0_f64.copysign(vx / wx)
    }
}

/// Python 3.13 `sum(iterable_of_floats)` (start `0`): the first item is `0 + item`, then the
/// Neumaier-compensated float loop of `bltinmodule.c builtin_sum_impl`
pub fn py_sum<'a>(items: impl IntoIterator<Item = &'a f64>) -> f64 {
    let mut it = items.into_iter();
    let Some(first) = it.next() else {
        return 0.0;
    };
    let mut f_result = 0.0 + *first;
    let mut c = 0.0_f64;
    for &x in it {
        let t = f_result + x;
        if f_result.abs() >= x.abs() {
            c += (f_result - t) + x;
        } else {
            c += (x - t) + f_result;
        }
        f_result = t;
    }
    if c != 0.0 && c.is_finite() {
        f_result += c;
    }
    f_result
}

/// `estimation.py:9 PrintTimeEstimator`
pub struct PrintTimeEstimator {
    /// `self.stats_weighing_until` (:17)
    pub stats_weighing_until: f64,
    /// `self.validity_range` (:20)
    pub validity_range: f64,
    /// `self.force_dumb_from_percent` (:23)
    pub force_dumb_from_percent: f64,
    /// `self.force_dumb_after_min` (:26)
    pub force_dumb_after_min: f64,
    /// `self._data` (:53)
    _data: TimeEstimationHelper,
}

impl PrintTimeEstimator {
    /// `estimation.py:16 __init__(job_type)`
    pub fn new(job_type: Option<&str>) -> PrintTimeEstimator {
        let stats_weighing_until = get_float(&["estimation", "printTime", "statsWeighingUntil"]);
        let validity_range = get_float(&["estimation", "printTime", "validityRange"]);
        let force_dumb_from_percent =
            get_float(&["estimation", "printTime", "forceDumbFromPercent"]);
        let force_dumb_after_min = get_float(&["estimation", "printTime", "forceDumbAfterMin"]);

        let mut threshold = None;
        let mut rolling_window = None;
        let mut countdown = None;

        if job_type == Some("local") || job_type == Some("sdcard") {
            // we are happy if the average of the estimates stays within 60s of the prior one
            threshold = Some(get_float(&["estimation", "printTime", "stableThreshold"]));

            if job_type == Some("sdcard") {
                // we are interesting in a rolling window of roughly the last 15s, so the number of entries has to be derived
                // by that divided by the sd status polling interval
                let mut interval = get_float(&["serial", "timeout", "sdStatus"]);
                if interval <= 0.0 {
                    interval = 1.0;
                }
                let floored = py_floordiv(15.0, interval);
                if !floored.is_finite() {
                    panic!("OverflowError: cannot convert float infinity to integer");
                }
                let mut window = floored as i64;
                if window < 1 {
                    window = 1;
                }
                rolling_window = Some(window);

                // we are happy when one rolling window has been stable
                countdown = Some(window);
            }
        }

        PrintTimeEstimator {
            stats_weighing_until,
            validity_range,
            force_dumb_from_percent,
            force_dumb_after_min,
            _data: TimeEstimationHelper::new(rolling_window, countdown, threshold),
        }
    }

    /// `estimation.py:57 estimate(progress, printTime, cleanedPrintTime, statisticalTotalPrintTime,
    /// statisticalTotalPrintTimeType)`: `(printTimeLeft, printTimeLeftOrigin)`
    pub fn estimate(
        &mut self,
        progress: Option<f64>,
        print_time: Option<f64>,
        cleaned_print_time: Option<f64>,
        statistical_total_print_time: Option<f64>,
        statistical_total_print_time_type: Option<&str>,
    ) -> Result<(Option<f64>, Option<String>), ZeroDivisionError> {
        let (Some(progress), Some(print_time), Some(cleaned_print_time)) =
            (progress, print_time, cleaned_print_time)
        else {
            return Ok((None, None));
        };
        if progress == 0.0 {
            return Ok((None, None));
        }

        // dumbTotalPrintTime = printTime / progress (progress is not 0 here)
        let dumb_total_print_time = print_time / progress;
        let estimated_total_print_time =
            self.estimate_total(Some(progress), Some(cleaned_print_time));
        let mut total_print_time = estimated_total_print_time;

        let mut print_time_left_origin = Some("estimate".to_string());
        if let Some(statistical_total) = statistical_total_print_time {
            match estimated_total_print_time {
                None => {
                    // no estimate yet, we'll use the statistical total
                    total_print_time = Some(statistical_total);
                    print_time_left_origin = statistical_total_print_time_type.map(str::to_string);
                }
                Some(estimated_total) => {
                    let sub_progress;
                    if progress < self.stats_weighing_until {
                        // still inside weighing range, use part stats, part current estimate
                        let mut sp = progress * (1.0 / self.stats_weighing_until);
                        if sp > 1.0 {
                            sp = 1.0;
                        }
                        sub_progress = sp;
                        print_time_left_origin = Some(format!(
                            "mixed-{}",
                            statistical_total_print_time_type.unwrap_or_else(|| panic!(
                                "TypeError: can only concatenate str (not \"NoneType\") to str"
                            ))
                        ));
                    } else {
                        // use only the current estimate
                        sub_progress = 1.0;
                        print_time_left_origin = Some("estimate".to_string());
                    }

                    // combine
                    total_print_time =
                        Some(-sub_progress * statistical_total + sub_progress * estimated_total);
                }
            }
        }

        let mut print_time_left = None;
        if let Some(total) = total_print_time {
            // sanity check current total print time estimate
            if total == 0.0 {
                return Err(ZeroDivisionError);
            }
            let assumed_progress = cleaned_print_time / total;
            let min_progress = progress - self.validity_range;
            let max_progress = progress + self.validity_range;

            if min_progress <= assumed_progress
                && assumed_progress <= max_progress
                && total > cleaned_print_time
            {
                // appears sane, we'll use it
                print_time_left = Some(total - cleaned_print_time);
            } else {
                // too far from the actual progress or negative,
                // we use the dumb print time instead
                print_time_left = Some(dumb_total_print_time - cleaned_print_time);
                print_time_left_origin = Some("linear".to_string());
            }
        } else {
            print_time_left_origin = Some("linear".to_string());
            if progress > self.force_dumb_from_percent
                || cleaned_print_time >= self.force_dumb_after_min * 60.0
            {
                // more than x% or y min printed and still no real estimate, ok, we'll use the dumb variant :/
                print_time_left = Some(dumb_total_print_time - cleaned_print_time);
            }
        }

        if let Some(left) = print_time_left
            && left < 0.0
        {
            // shouldn't actually happen, but let's make sure
            print_time_left = None;
        }

        Ok((print_time_left, print_time_left_origin))
    }

    /// `estimation.py:192 estimate_total(progress, printTime)`
    pub fn estimate_total(
        &mut self,
        progress: Option<f64>,
        print_time: Option<f64>,
    ) -> Option<f64> {
        // `not progress or not printTime or not self._data` (an object is truthy)
        let falsy = |v: Option<f64>| v.is_none_or(|x| x == 0.0);
        if falsy(progress) || falsy(print_time) {
            None
        } else {
            self._data.update(print_time.unwrap() / progress.unwrap())
        }
    }
}

/// `estimation.py:199 TimeEstimationHelper`
pub struct TimeEstimationHelper {
    /// `self._rolling_window` (:212)
    _rolling_window: i64,
    /// `self._countdown` (:213)
    _countdown: i64,
    /// `self._threshold` (:214)
    _threshold: f64,
    /// `self._distances` (:218), a `deque` of maxlen `_rolling_window`
    _distances: VecDeque<f64>,
    /// `self._totals` (:219)
    _totals: VecDeque<f64>,
    /// `self._sum_total` (:220), starts as the int `0`
    _sum_total: f64,
    /// `self._count` (:221)
    _count: i64,
    /// `self._stable_counter` (:222)
    _stable_counter: i64,
}

impl TimeEstimationHelper {
    /// `estimation.py:200 STABLE_THRESHOLD`
    pub const STABLE_THRESHOLD: f64 = 0.1;
    /// `estimation.py:201 STABLE_COUNTDOWN`
    pub const STABLE_COUNTDOWN: i64 = 250;
    /// `estimation.py:202 STABLE_ROLLING_WINDOW`
    pub const STABLE_ROLLING_WINDOW: i64 = 250;

    /// `estimation.py:204 __init__(rolling_window=None, countdown=None, threshold=None)`
    pub fn new(
        rolling_window: Option<i64>,
        countdown: Option<i64>,
        threshold: Option<f64>,
    ) -> TimeEstimationHelper {
        let rolling_window = rolling_window.unwrap_or(Self::STABLE_ROLLING_WINDOW);
        let countdown = countdown.unwrap_or(Self::STABLE_COUNTDOWN);
        let threshold = threshold.unwrap_or(Self::STABLE_THRESHOLD);

        TimeEstimationHelper {
            _rolling_window: rolling_window,
            _countdown: countdown,
            _threshold: threshold,
            _distances: VecDeque::new(),
            _totals: VecDeque::new(),
            _sum_total: 0.0,
            _count: 0,
            _stable_counter: -1,
        }
    }

    /// `deque.append` with `maxlen`
    fn append(deque: &mut VecDeque<f64>, maxlen: i64, value: f64) {
        if maxlen <= 0 {
            return;
        }
        if deque.len() as i64 >= maxlen {
            deque.pop_front();
        }
        deque.push_back(value);
    }

    /// `estimation.py:224 is_stable()`
    pub fn is_stable(&self) -> bool {
        self._stable_counter >= self._countdown
    }

    /// `estimation.py:227 update(new_estimate)`
    pub fn update(&mut self, new_estimate: f64) -> Option<f64> {
        let old_average_total = self.average_total();

        self._sum_total += new_estimate;
        Self::append(&mut self._totals, self._rolling_window, new_estimate);
        self._count += 1;

        if old_average_total != 0.0 {
            let distance = (self.average_total() - old_average_total).abs();
            Self::append(&mut self._distances, self._rolling_window, distance);
        }

        let average_distance = self.average_distance();
        if -self._threshold < average_distance && average_distance < self._threshold {
            self._stable_counter += 1;
        } else {
            self._stable_counter = -1;
        }

        if self.is_stable() {
            Some(self.average_total_rolling())
        } else {
            None
        }
    }

    /// `estimation.py:248 average_total`
    pub fn average_total(&self) -> f64 {
        if self._count == 0 {
            0.0
        } else {
            self._sum_total / self._count as f64
        }
    }

    /// `estimation.py:255 average_total_rolling`
    pub fn average_total_rolling(&self) -> f64 {
        if self._count == 0 || self._count < self._rolling_window || self._totals.is_empty() {
            -1.0
        } else {
            py_sum(self._totals.iter()) / self._totals.len() as f64
        }
    }

    /// `estimation.py:262 average_distance`
    pub fn average_distance(&self) -> f64 {
        if self._count == 0 || self._count < self._rolling_window + 1 || self._distances.is_empty()
        {
            -1.0
        } else {
            py_sum(self._distances.iter()) / self._distances.len() as f64
        }
    }
}

#[cfg(test)]
mod tests {
    //! The proof "result identical": the real `estimation.py` (Python 3.13, `python -I`, a stub
    //! `octoprint.settings.settings()` with the defaults) and this port get the same sequences;
    //! every `(printTimeLeft, origin)` must be identical, floats bit for bit.
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    const DRIVER: &str = r#"
import sys, json, struct, importlib.util, types
root = sys.argv[1]
class S:
    D = {("estimation","printTime","statsWeighingUntil"):0.5,("estimation","printTime","validityRange"):0.15,
         ("estimation","printTime","forceDumbFromPercent"):0.3,("estimation","printTime","forceDumbAfterMin"):30.0,
         ("estimation","printTime","stableThreshold"):60,("serial","timeout","sdStatus"):1.0}
    def getFloat(self, path): return float(self.D[tuple(path)])
m = types.ModuleType("octoprint"); ms = types.ModuleType("octoprint.settings")
ms.settings = lambda: S(); m.settings = ms
sys.modules["octoprint"] = m; sys.modules["octoprint.settings"] = ms
spec = importlib.util.spec_from_file_location("estimation", root + "/reference/OctoPrint-1.11.8/src/octoprint/printer/estimation.py")
est = importlib.util.module_from_spec(spec); spec.loader.exec_module(est)
def dec(h): return None if h == "n" else struct.unpack(">d", bytes.fromhex(h))[0]
def enc(x):
    if x is None: return "n"
    if isinstance(x, float): return "f" + struct.pack(">d", x).hex()
    return "i" + repr(x)
job = json.load(sys.stdin)
out = []
for run in job:
    e = est.PrintTimeEstimator(run["job_type"])
    res = []
    for st in run["steps"]:
        try:
            left, origin = e.estimate(dec(st[0]), dec(st[1]), dec(st[2]), dec(run["stat"]), run["stat_type"])
            res.append([enc(left), origin])
        except ZeroDivisionError:
            res.append(["zde", None])
    out.append(res)
json.dump(out, sys.stdout)
"#;

    fn hex(x: Option<f64>) -> String {
        match x {
            None => "n".to_string(),
            Some(x) => x
                .to_bits()
                .to_be_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    type Step = (Option<f64>, Option<f64>, Option<f64>);

    fn sequences() -> Vec<(&'static str, Vec<Step>)> {
        let n = 800;
        let mut stable = vec![];
        let mut noisy = vec![];
        let mut jumpy = vec![];
        let mut rng = Lcg(42);
        let mut t = 0.0;
        for i in 0..n {
            let progress = (i + 1) as f64 / (n + 20) as f64;
            let pt = 10.0 * (i + 1) as f64;
            stable.push((Some(progress), Some(pt), Some(pt - 5.0)));
            let noise = 1.0 + (rng.next() - 0.5) * 0.04;
            noisy.push((Some(progress), Some(pt * noise), Some(pt * noise - 5.0)));
            t += 10.0;
            let jump = if rng.next() < 0.05 {
                1.0 + rng.next() * 3.0
            } else {
                1.0
            };
            let p = if rng.next() < 0.03 {
                progress * (0.5 + rng.next())
            } else {
                progress
            };
            jumpy.push((
                Some(p.min(1.0)),
                Some(t * jump),
                Some((t * jump - 20.0).max(0.0)),
            ));
        }
        // the None / zero / one edges at the start of each
        let edges: Vec<Step> = vec![
            (None, None, None),
            (Some(0.0), Some(1.0), Some(1.0)),
            (Some(0.2), None, Some(1.0)),
            (Some(0.2), Some(1.0), None),
            (Some(1.0), Some(100.0), Some(90.0)),
        ];
        for s in [&mut stable, &mut noisy, &mut jumpy] {
            let mut with_edges = edges.clone();
            with_edges.append(s);
            *s = with_edges;
        }
        vec![("stable", stable), ("noisy", noisy), ("jumpy", jumpy)]
    }

    fn fmt_ours(left: Option<f64>) -> String {
        match left {
            None => "n".to_string(),
            Some(x) => format!("f{}", hex(Some(x))),
        }
    }

    #[test]
    fn estimator_identical_to_python() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let python = root.join("reference/.venv-octoprint-313/bin/python");
        let stats: [(Option<f64>, Option<&str>); 6] = [
            (None, None),
            (Some(8000.0), Some("average")),
            (Some(8200.0), Some("analysis")),
            (Some(300.0), Some("analysis")),
            (Some(100.0), Some("average")),
            (Some(50.0), Some("analysis")),
        ];
        let job_types: [Option<&str>; 4] = [Some("local"), Some("sdcard"), Some("stream"), None];
        let mut runs = vec![];
        let mut meta = vec![];
        for (name, steps) in sequences() {
            for jt in job_types {
                for (stat, stat_type) in stats {
                    runs.push(serde_json::json!({
                        "job_type": jt,
                        "stat": hex(stat),
                        "stat_type": stat_type,
                        "steps": steps.iter().map(|s| vec![hex(s.0), hex(s.1), hex(s.2)]).collect::<Vec<_>>(),
                    }));
                    meta.push((name, jt, stat, stat_type, steps.clone()));
                }
            }
        }
        let mut child = Command::new(&python)
            .arg("-I")
            .arg("-c")
            .arg(DRIVER)
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("could not start {python:?}: {e}"));
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&runs).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "python failed");
        let theirs: Vec<Vec<(String, Option<String>)>> =
            serde_json::from_slice(&output.stdout).unwrap();

        let mut compared = 0;
        let mut origins = std::collections::BTreeSet::new();
        for (run, (name, jt, stat, stat_type, steps)) in theirs.iter().zip(meta.iter()) {
            let mut e = PrintTimeEstimator::new(*jt);
            for (i, step) in steps.iter().enumerate() {
                let ours = match e.estimate(step.0, step.1, step.2, *stat, *stat_type) {
                    Ok((left, origin)) => (fmt_ours(left), origin),
                    Err(ZeroDivisionError) => ("zde".to_string(), None),
                };
                assert_eq!(run[i], ours, "{name} {jt:?} {stat:?} step {i} {step:?}");
                origins.insert(ours.1.clone());
                compared += 1;
            }
        }
        assert!(compared > 25000);
        // the proof reaches every origin the estimator can give
        for o in [
            "estimate",
            "linear",
            "average",
            "analysis",
            "mixed-average",
            "mixed-analysis",
        ] {
            assert!(
                origins.contains(&Some(o.to_string())),
                "origin {o} never reached"
            );
        }
        assert!(origins.contains(&None));
    }

    #[test]
    fn floordiv_and_sum_like_python() {
        assert_eq!(15.0, py_floordiv(15.0, 1.0));
        assert_eq!(3.0, py_floordiv(15.0, 4.0));
        assert_eq!(30.0, py_floordiv(15.0, 0.5));
        assert_eq!(py_sum([0.1, 0.2, 0.3].iter()), 0.6);
    }
}
