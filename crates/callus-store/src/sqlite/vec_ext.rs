//! One-time `sqlite-vec` auto-extension registration and vector serialization.

use std::sync::Once;

use rusqlite::ffi::sqlite3_auto_extension;
use sqlite_vec::sqlite3_vec_init;

static REGISTER_VEC: Once = Once::new();

/// Registers `sqlite-vec` for all subsequent SQLite connections in this process.
#[expect(unsafe_code, reason = "sqlite-vec C extension auto-registration FFI")]
pub(crate) fn register_sqlite_vec() {
    REGISTER_VEC.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the C entry point exported by `sqlite-vec`
        // matching SQLite's `sqlite3_auto_extension` function pointer signature.
        unsafe {
            type AutoExtFn = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int;
            let init: AutoExtFn = std::mem::transmute(sqlite3_vec_init as *const ());
            sqlite3_auto_extension(Some(init));
        }
    });
}

/// Encodes an `f32` embedding slice as little-endian bytes for `sqlite-vec`.
pub(crate) fn encode_f32_le(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}
