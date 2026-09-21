//! Query working directory of a Windows process via PEB.

// ========================================================================
// Functions
// ========================================================================

/// Query the working directory of a process by reading its PEB.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(super) fn query_process_cwd(pid: u32) -> Option<String> {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};

    type Handle = *mut c_void;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;

    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: i32,
        peb_base_address: usize,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetModuleHandleA(module_name: *const u8) -> Handle;
        fn GetProcAddress(module: Handle, proc_name: *const u8) -> *mut c_void;
        fn ReadProcessMemory(
            process: Handle,
            base_address: *const c_void,
            buffer: *mut c_void,
            size: usize,
            number_of_bytes_read: *mut usize,
        ) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }

        struct HandleGuard(Handle);
        impl Drop for HandleGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let _guard = HandleGuard(handle);

        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let nt_query_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if nt_query_ptr.is_null() {
            return None;
        }
        type NtQueryInformationProcessFn = unsafe extern "system" fn(
            process_handle: Handle,
            process_information_class: i32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(nt_query_ptr);

        let mut pbi: ProcessBasicInformation = zeroed();
        let mut return_length = 0u32;
        let status = nt_query(
            handle,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut c_void,
            size_of::<ProcessBasicInformation>() as u32,
            &mut return_length,
        );
        if status != 0 || pbi.peb_base_address == 0 {
            return None;
        }

        // On 64-bit Windows, RTL_USER_PROCESS_PARAMETERS pointer is at PEB + 0x20
        let mut process_parameters_addr: usize = 0;
        let mut bytes_read = 0usize;
        let read_ok = ReadProcessMemory(
            handle,
            (pbi.peb_base_address + 0x20) as *const c_void,
            &mut process_parameters_addr as *mut _ as *mut c_void,
            size_of::<usize>(),
            &mut bytes_read,
        );
        if read_ok == 0 || process_parameters_addr == 0 {
            return None;
        }

        // In RTL_USER_PROCESS_PARAMETERS, CurrentDirectory.DosPath is a UNICODE_STRING at offset 0x38
        let mut dos_path: UnicodeString = zeroed();
        let read_ok = ReadProcessMemory(
            handle,
            (process_parameters_addr + 0x38) as *const c_void,
            &mut dos_path as *mut _ as *mut c_void,
            size_of::<UnicodeString>(),
            &mut bytes_read,
        );
        if read_ok == 0 || dos_path.buffer.is_null() || dos_path.length == 0 {
            return None;
        }

        let char_count = (dos_path.length / 2) as usize;
        let mut buffer = vec![0u16; char_count];
        let read_ok = ReadProcessMemory(
            handle,
            dos_path.buffer as *const c_void,
            buffer.as_mut_ptr() as *mut c_void,
            dos_path.length as usize,
            &mut bytes_read,
        );
        if read_ok == 0 {
            return None;
        }

        let mut path_str = String::from_utf16_lossy(&buffer);
        if path_str.ends_with('\\') && path_str.len() > 3 {
            path_str.pop();
        }
        Some(path_str)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_query_process_cwd_reads_current_process() {
        let pid = std::process::id();
        let cwd = query_process_cwd(pid);
        assert!(cwd.is_some());
        let expected = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let expected_trimmed = expected.trim_end_matches('\\');
        let cwd_trimmed = cwd.unwrap();
        let cwd_trimmed = cwd_trimmed.trim_end_matches('\\');
        assert_eq!(cwd_trimmed.to_lowercase(), expected_trimmed.to_lowercase());
    }
}
