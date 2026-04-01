// arch/aarch64/syscall.rs — AArch64 SVC (syscall) entry point
//
// The exception vector calls `handle_svc` on EC=0x15 (SVC instruction).
// Syscall number is in x8; arguments in x0-x5; return value in x0.

use crate::arch::aarch64::exceptions::TrapFrame;

/// Dispatch a synchronous SVC exception to the Linux ABI syscall handler.
#[inline]
pub fn handle_svc(frame: &mut TrapFrame) {
    crate::syscall::handle(frame);
}
