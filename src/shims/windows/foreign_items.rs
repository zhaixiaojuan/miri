use std::iter;

use rustc_middle::mir;
use rustc_span::Symbol;
use rustc_target::abi::Size;
use rustc_target::spec::abi::Abi;

use crate::*;
use shims::foreign_items::EmulateByNameResult;
use shims::windows::sync::EvalContextExt as _;

impl<'mir, 'tcx: 'mir> EvalContextExt<'mir, 'tcx> for crate::MiriEvalContext<'mir, 'tcx> {}
pub trait EvalContextExt<'mir, 'tcx: 'mir>: crate::MiriEvalContextExt<'mir, 'tcx> {
    fn emulate_foreign_item_by_name(
        &mut self,
        link_name: Symbol,
        abi: Abi,
        args: &[OpTy<'tcx, Tag>],
        dest: &PlaceTy<'tcx, Tag>,
        _ret: mir::BasicBlock,
    ) -> InterpResult<'tcx, EmulateByNameResult<'mir, 'tcx>> {
        let this = self.eval_context_mut();

        // Windows API stubs.
        // HANDLE = isize
        // NTSTATUS = LONH = i32
        // DWORD = ULONG = u32
        // BOOL = i32
        // BOOLEAN = u8
        match &*link_name.as_str() {
            // Environment related shims
            "GetEnvironmentVariableW" => {
                let [name, buf, size] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.GetEnvironmentVariableW(name, buf, size)?;
                this.write_scalar(Scalar::from_u32(result), dest)?;
            }
            "SetEnvironmentVariableW" => {
                let [name, value] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.SetEnvironmentVariableW(name, value)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "GetEnvironmentStringsW" => {
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.GetEnvironmentStringsW()?;
                this.write_pointer(result, dest)?;
            }
            "FreeEnvironmentStringsW" => {
                let [env_block] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.FreeEnvironmentStringsW(env_block)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "GetCurrentDirectoryW" => {
                let [size, buf] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.GetCurrentDirectoryW(size, buf)?;
                this.write_scalar(Scalar::from_u32(result), dest)?;
            }
            "SetCurrentDirectoryW" => {
                let [path] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.SetCurrentDirectoryW(path)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }

            // Allocation
            "HeapAlloc" => {
                let [handle, flags, size] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(handle)?.to_machine_isize(this)?;
                let flags = this.read_scalar(flags)?.to_u32()?;
                let size = this.read_scalar(size)?.to_machine_usize(this)?;
                let zero_init = (flags & 0x00000008) != 0; // HEAP_ZERO_MEMORY
                let res = this.malloc(size, zero_init, MiriMemoryKind::WinHeap)?;
                this.write_pointer(res, dest)?;
            }
            "HeapFree" => {
                let [handle, flags, ptr] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(handle)?.to_machine_isize(this)?;
                this.read_scalar(flags)?.to_u32()?;
                let ptr = this.read_pointer(ptr)?;
                this.free(ptr, MiriMemoryKind::WinHeap)?;
                this.write_scalar(Scalar::from_i32(1), dest)?;
            }
            "HeapReAlloc" => {
                let [handle, flags, ptr, size] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(handle)?.to_machine_isize(this)?;
                this.read_scalar(flags)?.to_u32()?;
                let ptr = this.read_pointer(ptr)?;
                let size = this.read_scalar(size)?.to_machine_usize(this)?;
                let res = this.realloc(ptr, size, MiriMemoryKind::WinHeap)?;
                this.write_pointer(res, dest)?;
            }

            // errno
            "SetLastError" => {
                let [error] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let error = this.read_scalar(error)?.check_init()?;
                this.set_last_error(error)?;
            }
            "GetLastError" => {
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let last_error = this.get_last_error()?;
                this.write_scalar(last_error, dest)?;
            }

            // Querying system information
            "GetSystemInfo" => {
                let [system_info] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let system_info = this.deref_operand(system_info)?;
                // Initialize with `0`.
                this.write_bytes_ptr(
                    system_info.ptr,
                    iter::repeat(0u8).take(system_info.layout.size.bytes() as usize),
                )?;
                // Set number of processors.
                let dword_size = Size::from_bytes(4);
                let num_cpus = this.mplace_field(&system_info, 6)?;
                this.write_scalar(Scalar::from_int(NUM_CPUS, dword_size), &num_cpus.into())?;
            }

            // Thread-local storage
            "TlsAlloc" => {
                // This just creates a key; Windows does not natively support TLS destructors.

                // Create key and return it.
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let key = this.machine.tls.create_tls_key(None, dest.layout.size)?;
                this.write_scalar(Scalar::from_uint(key, dest.layout.size), dest)?;
            }
            "TlsGetValue" => {
                let [key] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let key = u128::from(this.read_scalar(key)?.to_u32()?);
                let active_thread = this.get_active_thread();
                let ptr = this.machine.tls.load_tls(key, active_thread, this)?;
                this.write_scalar(ptr, dest)?;
            }
            "TlsSetValue" => {
                let [key, new_ptr] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let key = u128::from(this.read_scalar(key)?.to_u32()?);
                let active_thread = this.get_active_thread();
                let new_data = this.read_scalar(new_ptr)?.check_init()?;
                this.machine.tls.store_tls(key, active_thread, new_data, &*this.tcx)?;

                // Return success (`1`).
                this.write_scalar(Scalar::from_i32(1), dest)?;
            }

            // Access to command-line arguments
            "GetCommandLineW" => {
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.write_pointer(
                    this.machine.cmd_line.expect("machine must be initialized").ptr,
                    dest,
                )?;
            }

            // Time related shims
            "GetSystemTimeAsFileTime" => {
                #[allow(non_snake_case)]
                let [LPFILETIME] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.GetSystemTimeAsFileTime(LPFILETIME)?;
            }
            "QueryPerformanceCounter" => {
                #[allow(non_snake_case)]
                let [lpPerformanceCount] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.QueryPerformanceCounter(lpPerformanceCount)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "QueryPerformanceFrequency" => {
                #[allow(non_snake_case)]
                let [lpFrequency] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let result = this.QueryPerformanceFrequency(lpFrequency)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }

            // Synchronization primitives
            "AcquireSRWLockExclusive" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.AcquireSRWLockExclusive(ptr)?;
            }
            "ReleaseSRWLockExclusive" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.ReleaseSRWLockExclusive(ptr)?;
            }
            "TryAcquireSRWLockExclusive" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let ret = this.TryAcquireSRWLockExclusive(ptr)?;
                this.write_scalar(Scalar::from_u8(ret), dest)?;
            }
            "AcquireSRWLockShared" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.AcquireSRWLockShared(ptr)?;
            }
            "ReleaseSRWLockShared" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.ReleaseSRWLockShared(ptr)?;
            }
            "TryAcquireSRWLockShared" => {
                let [ptr] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let ret = this.TryAcquireSRWLockShared(ptr)?;
                this.write_scalar(Scalar::from_u8(ret), dest)?;
            }

            // Dynamic symbol loading
            "GetProcAddress" => {
                #[allow(non_snake_case)]
                let [hModule, lpProcName] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(hModule)?.to_machine_isize(this)?;
                let name = this.read_c_str(this.read_pointer(lpProcName)?)?;
                if let Some(dlsym) = Dlsym::from_str(name, &this.tcx.sess.target.os)? {
                    let ptr = this.create_fn_alloc_ptr(FnVal::Other(dlsym));
                    this.write_pointer(ptr, dest)?;
                } else {
                    this.write_null(dest)?;
                }
            }

            // Miscellaneous
            "SystemFunction036" => {
                // This is really 'RtlGenRandom'.
                let [ptr, len] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let ptr = this.read_pointer(ptr)?;
                let len = this.read_scalar(len)?.to_u32()?;
                this.gen_random(ptr, len.into())?;
                this.write_scalar(Scalar::from_bool(true), dest)?;
            }
            "BCryptGenRandom" => {
                let [algorithm, ptr, len, flags] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let algorithm = this.read_scalar(algorithm)?;
                let ptr = this.read_pointer(ptr)?;
                let len = this.read_scalar(len)?.to_u32()?;
                let flags = this.read_scalar(flags)?.to_u32()?;
                if flags != 2 {
                    //      ^ BCRYPT_USE_SYSTEM_PREFERRED_RNG
                    throw_unsup_format!(
                        "BCryptGenRandom is supported only with the BCRYPT_USE_SYSTEM_PREFERRED_RNG flag"
                    );
                }
                if algorithm.to_machine_usize(this)? != 0 {
                    throw_unsup_format!(
                        "BCryptGenRandom algorithm must be NULL when the flag is BCRYPT_USE_SYSTEM_PREFERRED_RNG"
                    );
                }
                this.gen_random(ptr, len.into())?;
                this.write_null(dest)?; // STATUS_SUCCESS
            }
            "GetConsoleScreenBufferInfo" => {
                // `term` needs this, so we fake it.
                let [console, buffer_info] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(console)?.to_machine_isize(this)?;
                this.deref_operand(buffer_info)?;
                // Indicate an error.
                // FIXME: we should set last_error, but to what?
                this.write_null(dest)?;
            }
            "GetConsoleMode" => {
                // Windows "isatty" (in libtest) needs this, so we fake it.
                let [console, mode] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                this.read_scalar(console)?.to_machine_isize(this)?;
                this.deref_operand(mode)?;
                // Indicate an error.
                // FIXME: we should set last_error, but to what?
                this.write_null(dest)?;
            }
            "SwitchToThread" => {
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // Note that once Miri supports concurrency, this will need to return a nonzero
                // value if this call does result in switching to another thread.
                this.write_null(dest)?;
            }
            "GetStdHandle" => {
                let [which] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                let which = this.read_scalar(which)?.to_i32()?;
                // We just make this the identity function, so we know later in `NtWriteFile` which
                // one it is. This is very fake, but libtest needs it so we cannot make it a
                // std-only shim.
                this.write_scalar(Scalar::from_machine_isize(which.into(), this), dest)?;
            }

            // Better error for attempts to create a thread
            "CreateThread" => {
                let [_, _, _, _, _, _] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;

                this.handle_unsupported("can't create threads on Windows")?;
                return Ok(EmulateByNameResult::AlreadyJumped);
            }

            // Incomplete shims that we "stub out" just to get pre-main initialization code to work.
            // These shims are enabled only when the caller is in the standard library.
            "GetProcessHeap" if this.frame_in_std() => {
                let [] = this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // Just fake a HANDLE
                this.write_scalar(Scalar::from_machine_isize(1, this), dest)?;
            }
            "GetModuleHandleA" if this.frame_in_std() => {
                #[allow(non_snake_case)]
                let [_lpModuleName] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // We need to return something non-null here to make `compat_fn!` work.
                this.write_scalar(Scalar::from_machine_isize(1, this), dest)?;
            }
            "SetConsoleTextAttribute" if this.frame_in_std() => {
                #[allow(non_snake_case)]
                let [_hConsoleOutput, _wAttribute] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // Pretend these does not exist / nothing happened, by returning zero.
                this.write_null(dest)?;
            }
            "AddVectoredExceptionHandler" if this.frame_in_std() => {
                #[allow(non_snake_case)]
                let [_First, _Handler] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // Any non zero value works for the stdlib. This is just used for stack overflows anyway.
                this.write_scalar(Scalar::from_machine_usize(1, this), dest)?;
            }
            "SetThreadStackGuarantee" if this.frame_in_std() => {
                #[allow(non_snake_case)]
                let [_StackSizeInBytes] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                // Any non zero value works for the stdlib. This is just used for stack overflows anyway.
                this.write_scalar(Scalar::from_u32(1), dest)?;
            }
            | "InitializeCriticalSection"
            | "EnterCriticalSection"
            | "LeaveCriticalSection"
            | "DeleteCriticalSection"
                if this.frame_in_std() =>
            {
                #[allow(non_snake_case)]
                let [_lpCriticalSection] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                assert_eq!(
                    this.get_total_thread_count(),
                    1,
                    "concurrency on Windows is not supported"
                );
                // Nothing to do, not even a return value.
                // (Windows locks are reentrant, and we have only 1 thread,
                // so not doing any futher checks here is at least not incorrect.)
            }
            "TryEnterCriticalSection" if this.frame_in_std() => {
                #[allow(non_snake_case)]
                let [_lpCriticalSection] =
                    this.check_shim(abi, Abi::System { unwind: false }, link_name, args)?;
                assert_eq!(
                    this.get_total_thread_count(),
                    1,
                    "concurrency on Windows is not supported"
                );
                // There is only one thread, so this always succeeds and returns TRUE.
                this.write_scalar(Scalar::from_i32(1), dest)?;
            }

            _ => return Ok(EmulateByNameResult::NotSupported),
        }

        Ok(EmulateByNameResult::NeedsJumping)
    }
}
