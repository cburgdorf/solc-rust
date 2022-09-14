//! Rust bindings for the Solidity compiler.
//!
//! # Example
//! ```
//! pub fn main() {
//!     // Let input be a valid "Standard Solidity Input JSON"
//!     let input = "{}";
//!     let output = solc::compile(&input);
//!     assert_ne!(output.len(), 0);
//! }

#[macro_use]
extern crate lazy_static;

mod native;

use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::ptr;
use std::sync::Mutex;

/// Returns the compiler version string.
pub fn version() -> String {
    unsafe {
        CStr::from_ptr(native::solidity_version())
            .to_string_lossy()
            .into_owned()
    }
}

/// Returns the complete license text.
pub fn license() -> String {
    unsafe {
        CStr::from_ptr(native::solidity_license())
            .to_string_lossy()
            .into_owned()
    }
}

// Lock access to compiler
lazy_static! {
    static ref SOLC_MUTEX: Mutex<()> = Mutex::new(());
}

/// Compile using a valid JSON input and return a JSON output.
pub fn compile(input: &str) -> String {
    solidity_compile(input, None, std::ptr::null_mut())
}

/// Compile using a valid JSON input with read callback and return a JSON
/// output.
pub fn compile_with_callback<F>(input: &str, read_callback: F) -> String
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    // TODO: It should be possible to turn the box into a pointer without the
    // into_raw-from_raw dance
    let c_context = Box::into_raw(Box::new(read_callback));
    let result = solidity_compile(input, Some(call_callback::<F>), c_context as *mut ());
    unsafe { Box::from_raw(c_context) };
    result
}

fn solidity_compile(
    input: &str,
    callback: native::CStyleReadFileCallback,
    c_context: *mut (),
) -> String {
    let input_cstr: CString =
        CString::new(input).expect("CString failed (input contains a 0 byte?)");

    let _lock = SOLC_MUTEX
        .lock()
        .expect("Could not acquire exclusive access to the compiler");

    unsafe {
        let ptr = native::solidity_compile(input_cstr.as_ptr(), callback, c_context as *mut _);
        let output_cstr = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        native::solidity_free(ptr);
        output_cstr
    }
}

unsafe extern "C" fn call_callback<F>(
    c_context: *mut c_void,
    c_kind: *const c_char,
    c_data: *const c_char,
    o_contents: *mut *mut c_char,
    o_error: *mut *mut c_char,
) where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let callback_ptr = c_context as *mut F;
    let callback = &mut *callback_ptr;
    let kind = CStr::from_ptr(c_kind).to_string_lossy().into_owned();
    let data = CStr::from_ptr(c_data).to_string_lossy().into_owned();

    let result: Result<String, String> = callback(&kind, &data);
    match result {
        Ok(result) => copy_result_to_solidity_memory(&result, o_contents),
        Err(error) => copy_result_to_solidity_memory(&error, o_error),
    }
}

unsafe fn copy_result_to_solidity_memory(result: &str, target: *mut *mut c_char) {
    let contents_cstr: CString = CString::new(result).expect("Could not turn result into CString");
    let contents_size = contents_cstr.as_bytes_with_nul().len();

    // The solidity_reset() call in solidity_compile takes care of freeing the
    // memory alloc'd here
    let contents_ptr: *mut c_char = native::solidity_alloc(contents_size as u64);
    ptr::copy_nonoverlapping(contents_cstr.as_ptr(), contents_ptr, contents_size);
    (*target) = contents_ptr;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert_ne!(version().len(), 0);
    }

    #[test]
    fn test_license() {
        assert_ne!(license().len(), 0);
    }

    #[test]
    fn test_compile_smoke() {
        assert_ne!(compile("").len(), 0);
    }

    #[test]
    fn test_compile_single() {
        let input = r#"
        {
          "language": "Solidity",
          "settings": {
            "outputSelection": {
              "*": {
                "*": [ "evm.bytecode", "evm.gasEstimates" ]
              }
            }
          },
          "sources": {
            "c.sol": {
              "content": "contract C { function g() public { } function h() internal {} }"
            }
          }
        }
        "#;
        let output = compile(&input);
        // TODO: parse JSON and do a better job here
        assert_eq!(output.find("\"severity\":\"error\"").is_none(), true);
        assert_eq!(output.find("\"object\":\"").is_some(), true);
        assert_eq!(output.find(" CODECOPY ").is_some(), true);
    }

    #[test]
    fn test_compile_multi_missing() {
        let input = r#"
        {
          "language": "Solidity",
          "settings": {
            "outputSelection": {
              "*": {
                "*": [ "evm.bytecode", "evm.gasEstimates" ]
              }
            }
          },
          "sources": {
            "c.sol": {
              "content": "import \"d.sol\"; contract C is D { function g() public { } function h() internal {} }"
            }
          }
        }
        "#;
        let output = compile(&input);
        // TODO: parse JSON and do a better job here
        assert_eq!(output.find("\"severity\":\"error\"").is_none(), false);
        assert_eq!(output.find(" not found: ").is_some(), true);
    }

    #[test]
    fn test_compile_multi_with_callback() {
        let input = r#"
        {
          "language": "Solidity",
          "settings": {
            "outputSelection": {
              "*": {
                "*": [ "evm.bytecode", "evm.gasEstimates" ]
              }
            }
          },
          "sources": {
            "c.sol": {
              "content": "import \"d.sol\"; contract C is D { function g() public { } function h() internal {} }"
            }
          }
        }
        "#;
        let output =
            compile_with_callback(&input, |kind: &str, data: &str| -> Result<String, String> {
                assert_eq!(data, "d.sol");
                assert_eq!(kind, "source");
                Ok("contract D {}".to_string())
            });
        // TODO: parse JSON and do a better job here
        assert_eq!(output.find("\"severity\":\"error\"").is_none(), true);
        assert_eq!(output.find("\"object\":\"").is_some(), true);
        assert_eq!(output.find(" CODECOPY ").is_some(), true);
    }

    #[test]
    fn test_compile_multi_with_failing_callback() {
        let input = r#"
        {
          "language": "Solidity",
          "settings": {
            "outputSelection": {
              "*": {
                "*": [ "evm.bytecode", "evm.gasEstimates" ]
              }
            }
          },
          "sources": {
            "c.sol": {
              "content": "import \"d.sol\"; contract C is D { function g() public { } function h() internal {} }"
            }
          }
        }
        "#;
        let output =
            compile_with_callback(&input, |kind: &str, data: &str| -> Result<String, String> {
                Err("Our apologies".to_string())
            });
        // TODO: parse JSON and do a better job here
        assert_eq!(output.find("\"severity\":\"error\"").is_none(), false);
        assert_eq!(output.find("apologies").is_some(), true);
    }
}
