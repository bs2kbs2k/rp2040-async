// Adapted from rp-hal
// MIT License Copyright (c) 2021 rp-rs organization
// Run a function on the second thread

use core::{
    mem::ManuallyDrop,
    panic,
    sync::atomic::{compiler_fence, Ordering},
};

/// Data type for a properly aligned stack of N 32-bit (usize) words
#[repr(C, align(32))]
pub struct Stack<const SIZE: usize> {
    /// Memory to be used for the stack
    pub mem: [usize; SIZE],
}

impl<const SIZE: usize> Stack<SIZE> {
    /// Construct a stack of length SIZE, initialized to 0
    pub const fn new() -> Stack<SIZE> {
        Stack { mem: [0; SIZE] }
    }
}

static mut STACK: Stack<4096> = Stack::new();

/// Spawn a function on this core.
pub fn spawn<F>(entry: F)
where
    F: FnOnce() -> ! + Send + 'static,
{
    // The first two ignored `u64` parameters are there to take up all of the registers,
    // which means that the rest of the arguments are taken from the stack,
    // where we're able to put them from core 0.
    extern "C" fn core1_startup<F: FnOnce() -> !>(
        _: u64,
        _: u64,
        entry: &mut ManuallyDrop<F>,
        stack_bottom: *mut usize,
    ) -> ! {
        let core = unsafe { rp2040_pac::CorePeripherals::steal() };

        // Trap if MPU is already configured
        if core.MPU.ctrl.read() != 0 {
            cortex_m::asm::udf();
        }

        // The minimum we can protect is 32 bytes on a 32 byte boundary, so round up which will
        // just shorten the valid stack range a tad.
        let addr = (stack_bottom as u32 + 31) & !31;
        // Mask is 1 bit per 32 bytes of the 256 byte range... clear the bit for the segment we want
        let subregion_select = 0xff ^ (1 << ((addr >> 5) & 7));
        unsafe {
            core.MPU.ctrl.write(5); // enable mpu with background default map
            core.MPU.rbar.write((addr & !0xff) | 0x8);
            core.MPU.rasr.write(
                1 // enable region
               | (0x7 << 1) // size 2^(7 + 1) = 256
               | (subregion_select << 8)
               | 0x10000000, // XN = disable instruction fetch; no other bits means no permissions
            );
        }

        let entry = unsafe { ManuallyDrop::take(entry) };

        // Signal that it's safe for core 0 to get rid of the original value now.
        //
        // We don't have any way to get at core 1's SIO without using `Peripherals::steal` right now,
        // since svd2rust doesn't really support multiple cores properly.
        let sio = unsafe { &(*rp2040_pac::SIO::ptr()) };
        while !sio.fifo_st.read().rdy().bit_is_set() {
            cortex_m::asm::nop();
        }
        sio.fifo_wr.write(|w| unsafe { w.bits(1) });
        cortex_m::asm::sev();

        entry()
    }

    let psm = unsafe { &(*rp2040_pac::PSM::ptr()) };

    // Reset the core
    psm.frce_off.modify(|_, w| w.proc1().set_bit());
    while !psm.frce_off.read().proc1().bit_is_set() {
        cortex_m::asm::nop();
    }
    psm.frce_off.modify(|_, w| w.proc1().clear_bit());

    let stack = unsafe { &mut STACK.mem };

    // Set up the stack
    let mut stack_ptr = unsafe { stack.as_mut_ptr().add(stack.len()) };

    // We don't want to drop this, since it's getting moved to the other core.
    let mut entry = ManuallyDrop::new(entry);

    // Push the arguments to `core1_startup` onto the stack.
    unsafe {
        // Push `stack_bottom`.
        stack_ptr = stack_ptr.sub(1);
        stack_ptr.cast::<*mut usize>().write(stack.as_mut_ptr());

        // Push `entry`.
        stack_ptr = stack_ptr.sub(1);
        stack_ptr.cast::<&mut ManuallyDrop<F>>().write(&mut entry);
    }

    // Make sure the compiler does not reorder the stack writes after to after the
    // below FIFO writes, which would result in them not being seen by the second
    // core.
    //
    // From the compiler perspective, this doesn't guarantee that the second core
    // actually sees those writes. However, we know that the RP2040 doesn't have
    // memory caches, and writes happen in-order.
    compiler_fence(Ordering::Release);

    let ppb = unsafe { &(*rp2040_pac::PPB::ptr()) };
    let vector_table = ppb.vtor.read().bits();

    // After reset, core 1 is waiting to receive commands over FIFO.
    // This is the sequence to have it jump to some code.
    let cmd_seq = [
        0,
        0,
        1,
        vector_table as usize,
        stack_ptr as usize,
        core1_startup::<F> as usize,
    ];

    let mut seq = 0;
    let mut fails = 0;
    let sio = unsafe { &(*rp2040_pac::SIO::ptr()) };
    loop {
        let cmd = cmd_seq[seq] as u32;
        if cmd == 0 {
            while sio.fifo_st.read().vld().bit_is_set() {
                let _ = sio.fifo_rd.read().bits();
            }
            cortex_m::asm::sev();
        }
        while !sio.fifo_st.read().rdy().bit_is_set() {
            cortex_m::asm::nop();
        }
        sio.fifo_wr.write(|w| unsafe { w.bits(cmd) });
        cortex_m::asm::sev();

        let response = loop {
            // Have we got something?
            if sio.fifo_st.read().vld().bit_is_set() {
                // Yes, return it right away
                break sio.fifo_rd.read().bits();
            } else {
                // No, so sleep the CPU. We expect the sending core to `sev`
                // on write.
                cortex_m::asm::wfe();
            }
        };
        if cmd == response {
            seq += 1;
        } else {
            seq = 0;
            fails += 1;
            if fails > 16 {
                // The second core isn't responding, and isn't going to take the entrypoint,
                // so we have to drop it ourselves.
                drop(ManuallyDrop::into_inner(entry));
                panic!();
            }
        }
        if seq >= cmd_seq.len() {
            break;
        }
    }

    // Wait until the other core has copied `entry` before returning.
    loop {
        // Have we got something?
        if sio.fifo_st.read().vld().bit_is_set() {
            // Yes, return it right away
            let _ = sio.fifo_rd.read().bits();
            break;
        } else {
            // No, so sleep the CPU. We expect the sending core to `sev`
            // on write.
            cortex_m::asm::wfe();
        }
    }
}
