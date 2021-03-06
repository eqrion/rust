use core::sync::atomic::{AtomicUsize, Ordering};
use io::Write;

// runtime features
mod reloc;
pub(super) mod panic;

// library features
pub mod mem;
pub mod thread;
pub mod tls;
#[macro_use]
pub mod usercalls;

global_asm!(concat!(usercalls_asm!(), include_str!("entry.S")));

#[no_mangle]
unsafe extern "C" fn tcs_init(secondary: bool) {
    // Be very careful when changing this code: it runs before the binary has been
    // relocated. Any indirect accesses to symbols will likely fail.
    const UNINIT: usize = 0;
    const BUSY: usize = 1;
    const DONE: usize = 2;
    // Three-state spin-lock
    static RELOC_STATE: AtomicUsize = AtomicUsize::new(UNINIT);

    if secondary && RELOC_STATE.load(Ordering::Relaxed) != DONE {
        panic::panic_msg("Entered secondary TCS before main TCS!")
    }

    // Try to atomically swap UNINIT with BUSY. The returned state can be:
    match RELOC_STATE.compare_and_swap(UNINIT, BUSY, Ordering::Acquire) {
        // This thread just obtained the lock and other threads will observe BUSY
        UNINIT => {
            reloc::relocate_elf_rela();
            RELOC_STATE.store(DONE, Ordering::Release);
        },
        // We need to wait until the initialization is done.
        BUSY => while RELOC_STATE.load(Ordering::Acquire) == BUSY  {
            ::core::arch::x86_64::_mm_pause()
        },
        // Initialization is done.
        DONE => {},
        _ => unreachable!()
    }
}

// FIXME: this item should only exist if this is linked into an executable
// (main function exists). If this is a library, the crate author should be
// able to specify this
#[no_mangle]
extern "C" fn entry(p1: u64, p2: u64, p3: u64, secondary: bool, p4: u64, p5: u64) -> (u64, u64) {
    // FIXME: how to support TLS in library mode?
    let tls = Box::new(tls::Tls::new());
    let _tls_guard = unsafe { tls.activate() };

    if secondary {
        super::thread::Thread::entry();

        (0, 0)
    } else {
        extern "C" {
            fn main(argc: isize, argv: *const *const u8) -> isize;
        }

        // check entry is being called according to ABI
        assert_eq!(p3, 0);
        assert_eq!(p4, 0);
        assert_eq!(p5, 0);

        unsafe {
            // The actual types of these arguments are `p1: *const Arg, p2:
            // usize`. We can't currently customize the argument list of Rust's
            // main function, so we pass these in as the standard pointer-sized
            // values in `argc` and `argv`.
            let ret = main(p2 as _, p1 as _);
            exit_with_code(ret)
        }
    }
}

pub(super) fn exit_with_code(code: isize) -> ! {
    if code != 0 {
        if let Some(mut out) = panic::SgxPanicOutput::new() {
            let _ = write!(out, "Exited with status code {}", code);
        }
    }
    usercalls::exit(code != 0);
}
