//! Ours (W9): the filter's stderr. The driver writes `DEBUG:`, `INFO:`, `STATE:`, `PAGE:`, `ERROR:` and
//! `WARNING:` lines with `fputs`/`fprintf(stderr, ...)`; CUPS's scheduler reads them. A filter run is one
//! "process" on one thread, so its stderr is a thread-local list of lines, which the port reads, and an
//! optional callback that sees each line as it is written.

use std::cell::RefCell;

type Callback = Box<dyn FnMut(&str)>;

thread_local! {
    static LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CALLBACK: RefCell<Option<Callback>> = const { RefCell::new(None) };
}

/// `fputs(text, stderr)` / `fprintf(stderr, ...)`: text as the driver writes it, line ends included.
pub fn write(text: &str) {
    LINES.with(|l| l.borrow_mut().push(text.to_owned()));
    CALLBACK.with(|c| {
        if let Some(f) = c.borrow_mut().as_mut() {
            f(text);
        }
    });
}

/// Starts a new "process": the lines so far are dropped, `callback` sees each new one.
pub fn start(callback: Option<Callback>) {
    LINES.with(|l| l.borrow_mut().clear());
    CALLBACK.with(|c| *c.borrow_mut() = callback);
}

/// Everything written since [`start`], and stops the callback.
pub fn take() -> Vec<String> {
    CALLBACK.with(|c| *c.borrow_mut() = None);
    LINES.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// The lines so far (for the tests).
pub fn lines() -> Vec<String> {
    LINES.with(|l| l.borrow().clone())
}
