use core_foundation_sys::{
    array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{kCFAllocatorDefault, CFGetTypeID, CFRelease, CFTypeRef},
    dictionary::{
        CFDictionaryGetCount, CFDictionaryGetKeysAndValues, CFDictionaryGetValue, CFDictionaryRef,
    },
    number::{kCFNumberSInt32Type, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef},
    string::{
        kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString, CFStringGetTypeID,
        CFStringRef,
    },
};
use std::{ffi::CString, os::raw::c_void, ptr};

const K_IO_RETURN_SUCCESS: i32 = 0;
const RELEVANT_TYPES: [&str; 2] = ["NoIdleSleepAssertion", "NoDisplaySleepAssertion"];

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMCopyAssertionsByProcess(assertions_by_pid: *mut CFDictionaryRef) -> i32;
}

pub struct ProcessAssertions {
    pub process_id: i32,
    pub assertion_types: Vec<String>,
}

struct OwnedCf(CFTypeRef);

impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

unsafe fn cf_string(value: CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut buffer = vec![0_i8; 256];
    if CFStringGetCString(
        value,
        buffer.as_mut_ptr(),
        buffer.len() as isize,
        kCFStringEncodingUTF8,
    ) == 0
    {
        return None;
    }
    Some(
        std::ffi::CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned(),
    )
}

unsafe fn dictionary_key(name: &str) -> Result<(CFStringRef, OwnedCf), String> {
    let name = CString::new(name).map_err(|error| error.to_string())?;
    let key = CFStringCreateWithCString(kCFAllocatorDefault, name.as_ptr(), kCFStringEncodingUTF8);
    if key.is_null() {
        return Err("Failed to allocate Core Foundation dictionary key".into());
    }
    Ok((key, OwnedCf(key as CFTypeRef)))
}

unsafe fn process_id(key: *const c_void) -> Option<i32> {
    let type_id = CFGetTypeID(key as CFTypeRef);
    if type_id == CFNumberGetTypeID() {
        let mut process_id = 0_i32;
        return CFNumberGetValue(
            key as CFNumberRef,
            kCFNumberSInt32Type,
            &mut process_id as *mut _ as *mut c_void,
        )
        .then_some(process_id);
    }

    if type_id == CFStringGetTypeID() {
        return cf_string(key as CFStringRef)?.parse().ok();
    }

    None
}

unsafe fn read_assertion(assertion: CFDictionaryRef) -> Result<Option<String>, String> {
    // Literal values behind kIOPMAssertionTypeKey and
    // kIOPMAssertionLevelKey in IOPMLib.h.
    let (type_key, _type_guard) = dictionary_key("AssertType")?;
    let (level_key, _level_guard) = dictionary_key("AssertLevel")?;
    let assertion_type = CFDictionaryGetValue(assertion, type_key as *const c_void) as CFStringRef;
    let level = CFDictionaryGetValue(assertion, level_key as *const c_void) as CFNumberRef;

    let assertion_type = match cf_string(assertion_type) {
        Some(value) if RELEVANT_TYPES.contains(&value.as_str()) => value,
        _ => return Ok(None),
    };

    let mut level_value = 0_i32;
    if level.is_null()
        || !CFNumberGetValue(
            level,
            kCFNumberSInt32Type,
            &mut level_value as *mut _ as *mut c_void,
        )
        || level_value == 0
    {
        return Ok(None);
    }
    Ok(Some(assertion_type))
}

pub fn active_no_sleep_assertions() -> Result<Vec<ProcessAssertions>, String> {
    unsafe {
        let mut assertions: CFDictionaryRef = ptr::null();
        let result = IOPMCopyAssertionsByProcess(&mut assertions);
        if result != K_IO_RETURN_SUCCESS || assertions.is_null() {
            return Err(format!("IOPMCopyAssertionsByProcess returned {}", result));
        }
        let _assertions_guard = OwnedCf(assertions as CFTypeRef);

        let count = CFDictionaryGetCount(assertions);
        let mut keys = vec![ptr::null(); count as usize];
        let mut values = vec![ptr::null(); count as usize];
        CFDictionaryGetKeysAndValues(assertions, keys.as_mut_ptr(), values.as_mut_ptr());

        let mut matches = Vec::new();
        for index in 0..count as usize {
            let Some(process_id) = process_id(keys[index]) else {
                continue;
            };

            let assertion_array = values[index] as CFArrayRef;
            let mut assertion_types = Vec::new();
            for assertion_index in 0..CFArrayGetCount(assertion_array) {
                let assertion =
                    CFArrayGetValueAtIndex(assertion_array, assertion_index) as CFDictionaryRef;
                if let Some(assertion_type) = read_assertion(assertion)? {
                    if !assertion_types.contains(&assertion_type) {
                        assertion_types.push(assertion_type);
                    }
                }
            }
            if !assertion_types.is_empty() {
                matches.push(ProcessAssertions {
                    process_id,
                    assertion_types,
                });
            }
        }
        Ok(matches)
    }
}
