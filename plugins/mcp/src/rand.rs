//! Random bytes from the operating system (no crate): `BCryptGenRandom` on Windows, `/dev/urandom` elsewhere.

#[cfg(windows)]
fn fill(buf: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    extern "system" { fn BCryptGenRandom(alg: *mut core::ffi::c_void, buf: *mut u8, len: u32, flags: u32) -> i32; }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
    unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) == 0 }
}
#[cfg(not(windows))]
fn fill(buf: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).is_ok()
}

/// `n` random bytes as lower-case hex; None when the system has no randomness to give.
pub fn hex(n: usize) -> Option<String> {
    let mut b = vec![0u8; n];
    fill(&mut b).then(|| b.iter().map(|x| format!("{x:02x}")).collect())
}
