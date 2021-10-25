use core::panic::PanicInfo;

#[cfg(debug_assertions)]
use core::sync::atomic::{self, Ordering};

#[cfg(debug_assertions)]
#[inline(never)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log::error!("PANIC {}", info);
    loop {
        atomic::compiler_fence(Ordering::SeqCst);
    }
}

#[cfg(not(debug_assertions))]
#[inline(never)]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}
