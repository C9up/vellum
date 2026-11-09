//! Where hayro reads an untrusted document.
//!
//! hayro bounds its recursion by depth (512 nested objects, 50 nested
//! interpretations, 64 nested colour spaces or functions), and how much stack
//! those depths need is fixed by the code, not by the input. The thread the
//! call arrives on is not: a libuv worker, Node's main thread, or a 1 MiB
//! Windows thread. The work runs on a thread of our own whose stack is sized
//! for those depths, so the bounds hold wherever the call comes from.

use std::any::Any;

/// Comfortably above what the depth limits need in a debug build.
const STACK_BYTES: usize = 16 * 1024 * 1024;

/// Run `work` on a reader thread and wait for it.
///
/// A panic stays an error: it is caught at the join, the same as the binding
/// would catch it on the calling thread.
pub(crate) fn on_reader_thread<T, F>(work: F) -> Result<T, String>
where
    T: Send,
    F: FnOnce() -> Result<T, String> + Send,
{
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("vellum-reader".to_string())
            .stack_size(STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(|error| format!("cannot start a reader thread: {error}"))?;
        handle
            .join()
            .unwrap_or_else(|payload| Err(panic_message(&payload)))
    })
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown cause".to_string());
    format!("internal error while reading the PDF: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_on_the_reader_thread_comes_back_as_an_error() {
        let result: Result<(), String> = on_reader_thread(|| panic!("boom"));
        let message = result.expect_err("the panic should surface as an error");
        assert!(message.contains("boom"), "got {message}");
    }

    #[test]
    fn the_result_is_handed_back() {
        let bytes = [1u8, 2, 3];
        let sum = on_reader_thread(|| Ok(bytes.iter().map(|&b| u32::from(b)).sum::<u32>()));
        assert_eq!(sum, Ok(6));
    }
}
