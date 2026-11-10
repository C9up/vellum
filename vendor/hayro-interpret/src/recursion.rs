//! vellum: a colour space names another colour space, and a function other
//! functions, each through its own indirect object. Nothing bounded the
//! chain, so 100 000 distinct objects each naming the next recursed once per
//! object and overflowed the stack, which ends the process rather than
//! returning `None`. Real documents nest a few levels.

use std::cell::Cell;

const MAX_DEPTH: u32 = 64;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// One level of a recursive construction; `None` past the limit.
pub(crate) struct Level(());

impl Level {
    pub(crate) fn enter() -> Option<Self> {
        DEPTH.with(|depth| {
            if depth.get() >= MAX_DEPTH {
                // vellum: remembered, so the document is refused rather
                // than drawn without it.
                hayro_syntax::budget::note_too_deep(
                    "colour spaces or functions more than 64 levels deep",
                );
                return None;
            }
            depth.set(depth.get() + 1);
            Some(Self(()))
        })
    }
}

impl Drop for Level {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}
