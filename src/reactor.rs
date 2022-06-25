use core::task::Waker;

extern crate alloc;

use alloc::vec::Vec;
use cortex_m_rt::exception;

use crate::sync::Mutex;

type WakerList = Vec<Waker>;
const WAKER_LIST: WakerList = WakerList::new();
pub static WAKERS: Mutex<[WakerList; 26], 7> = Mutex::new([WAKER_LIST; 26]);

#[exception]
unsafe fn DefaultHandler(irqn: i16) {
    if irqn < 0 {
        // Not an interrupt; return immediately.
        return;
    } else {
        // Interrupt; handle it.
        let mut wakers = WAKERS.lock();
        let waker_list = wakers[irqn as usize];
        for waker in waker_list.iter() {
            waker.wake();
        }
    }
}
