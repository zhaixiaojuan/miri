use std::ffi::OsStr;

use log::trace;

use rustc_middle::mir;
use rustc_middle::ty::layout::LayoutOf;
use rustc_span::Symbol;
use rustc_target::abi::{Align, Size};
use rustc_target::spec::abi::Abi;

use crate::*;
use shims::foreign_items::EmulateByNameResult;
use shims::posix::fs::EvalContextExt as _;
use shims::posix::sync::EvalContextExt as _;
use shims::posix::thread::EvalContextExt as _;

impl<'mir, 'tcx: 'mir> EvalContextExt<'mir, 'tcx> for crate::MiriEvalContext<'mir, 'tcx> {}
pub trait EvalContextExt<'mir, 'tcx: 'mir>: crate::MiriEvalContextExt<'mir, 'tcx> {
    fn emulate_foreign_item_by_name(
        &mut self,
        link_name: Symbol,
        abi: Abi,
        args: &[OpTy<'tcx, Tag>],
        dest: &PlaceTy<'tcx, Tag>,
        ret: mir::BasicBlock,
    ) -> InterpResult<'tcx, EmulateByNameResult<'mir, 'tcx>> {
        let this = self.eval_context_mut();

        match &*link_name.as_str() {
            // Environment related shims
            "getenv" => {
                let [name] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.getenv(name)?;
                this.write_pointer(result, dest)?;
            }
            "unsetenv" => {
                let [name] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.unsetenv(name)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "setenv" => {
                let [name, value, overwrite] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.read_scalar(overwrite)?.to_i32()?;
                let result = this.setenv(name, value)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "getcwd" => {
                let [buf, size] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.getcwd(buf, size)?;
                this.write_pointer(result, dest)?;
            }
            "chdir" => {
                let [path] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.chdir(path)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }

            // File related shims
            "open" | "open64" => {
                // `open` is variadic, the third argument is only present when the second argument has O_CREAT (or on linux O_TMPFILE, but miri doesn't support that) set
                this.check_abi_and_shim_symbol_clash(abi, Abi::C { unwind: false }, link_name)?;
                let result = this.open(args)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "fcntl" => {
                // `fcntl` is variadic. The argument count is checked based on the first argument
                // in `this.fcntl()`, so we do not use `check_shim` here.
                this.check_abi_and_shim_symbol_clash(abi, Abi::C { unwind: false }, link_name)?;
                let result = this.fcntl(args)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "read" => {
                let [fd, buf, count] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let fd = this.read_scalar(fd)?.to_i32()?;
                let buf = this.read_pointer(buf)?;
                let count = this.read_scalar(count)?.to_machine_usize(this)?;
                let result = this.read(fd, buf, count)?;
                this.write_scalar(Scalar::from_machine_isize(result, this), dest)?;
            }
            "write" => {
                let [fd, buf, n] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let fd = this.read_scalar(fd)?.to_i32()?;
                let buf = this.read_pointer(buf)?;
                let count = this.read_scalar(n)?.to_machine_usize(this)?;
                trace!("Called write({:?}, {:?}, {:?})", fd, buf, count);
                let result = this.write(fd, buf, count)?;
                // Now, `result` is the value we return back to the program.
                this.write_scalar(Scalar::from_machine_isize(result, this), dest)?;
            }
            "unlink" => {
                let [path] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.unlink(path)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "symlink" => {
                let [target, linkpath] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.symlink(target, linkpath)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "rename" => {
                let [oldpath, newpath] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.rename(oldpath, newpath)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "mkdir" => {
                let [path, mode] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.mkdir(path, mode)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "rmdir" => {
                let [path] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.rmdir(path)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "closedir" => {
                let [dirp] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.closedir(dirp)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "lseek" | "lseek64" => {
                let [fd, offset, whence] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.lseek64(fd, offset, whence)?;
                // "lseek" is only used on macOS which is 64bit-only, so `i64` always works.
                this.write_scalar(Scalar::from_i64(result), dest)?;
            }
            "fsync" => {
                let [fd] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.fsync(fd)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "fdatasync" => {
                let [fd] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.fdatasync(fd)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "readlink" => {
                let [pathname, buf, bufsize] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.readlink(pathname, buf, bufsize)?;
                this.write_scalar(Scalar::from_machine_isize(result, this), dest)?;
            }

            // Allocation
            "posix_memalign" => {
                let [ret, align, size] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let ret = this.deref_operand(ret)?;
                let align = this.read_scalar(align)?.to_machine_usize(this)?;
                let size = this.read_scalar(size)?.to_machine_usize(this)?;
                // Align must be power of 2, and also at least ptr-sized (POSIX rules).
                if !align.is_power_of_two() {
                    throw_ub_format!("posix_memalign: alignment must be a power of two, but is {}", align);
                }
                if align < this.pointer_size().bytes() {
                    throw_ub_format!(
                        "posix_memalign: alignment must be at least the size of a pointer, but is {}",
                        align,
                    );
                }

                if size == 0 {
                    this.write_null(&ret.into())?;
                } else {
                    let ptr = this.allocate_ptr(
                        Size::from_bytes(size),
                        Align::from_bytes(align).unwrap(),
                        MiriMemoryKind::C.into(),
                    )?;
                    this.write_pointer(ptr, &ret.into())?;
                }
                this.write_null(dest)?;
            }

            // Dynamic symbol loading
            "dlsym" => {
                let [handle, symbol] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.read_scalar(handle)?.to_machine_usize(this)?;
                let symbol = this.read_pointer(symbol)?;
                let symbol_name = this.read_c_str(symbol)?;
                if let Some(dlsym) = Dlsym::from_str(symbol_name, &this.tcx.sess.target.os)? {
                    let ptr = this.create_fn_alloc_ptr(FnVal::Other(dlsym));
                    this.write_pointer(ptr, dest)?;
                } else {
                    this.write_null(dest)?;
                }
            }

            // Querying system information
            "sysconf" => {
                let [name] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let name = this.read_scalar(name)?.to_i32()?;

                let sysconfs = &[
                    ("_SC_PAGESIZE", Scalar::from_int(PAGE_SIZE, this.pointer_size())),
                    ("_SC_NPROCESSORS_CONF", Scalar::from_int(NUM_CPUS, this.pointer_size())),
                    ("_SC_NPROCESSORS_ONLN", Scalar::from_int(NUM_CPUS, this.pointer_size())),
                ];
                let mut result = None;
                for &(sysconf_name, value) in sysconfs {
                    let sysconf_name = this.eval_libc_i32(sysconf_name)?;
                    if sysconf_name == name {
                        result = Some(value);
                        break;
                    }
                }
                if let Some(result) = result {
                    this.write_scalar(result, dest)?;
                } else {
                    throw_unsup_format!("unimplemented sysconf name: {}", name)
                }
            }

            // Thread-local storage
            "pthread_key_create" => {
                let [key, dtor] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let key_place = this.deref_operand(key)?;
                let dtor = this.read_pointer(dtor)?;

                // Extract the function type out of the signature (that seems easier than constructing it ourselves).
                let dtor = if !this.ptr_is_null(dtor)? {
                    Some(this.get_ptr_fn(dtor)?.as_instance()?)
                } else {
                    None
                };

                // Figure out how large a pthread TLS key actually is.
                // To this end, deref the argument type. This is `libc::pthread_key_t`.
                let key_type = key.layout.ty
                    .builtin_deref(true)
                    .ok_or_else(|| err_ub_format!(
                        "wrong signature used for `pthread_key_create`: first argument must be a raw pointer."
                    ))?
                    .ty;
                let key_layout = this.layout_of(key_type)?;

                // Create key and write it into the memory where `key_ptr` wants it.
                let key = this.machine.tls.create_tls_key(dtor, key_layout.size)?;
                this.write_scalar(Scalar::from_uint(key, key_layout.size), &key_place.into())?;

                // Return success (`0`).
                this.write_null(dest)?;
            }
            "pthread_key_delete" => {
                let [key] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let key = this.read_scalar(key)?.check_init()?.to_bits(key.layout.size)?;
                this.machine.tls.delete_tls_key(key)?;
                // Return success (0)
                this.write_null(dest)?;
            }
            "pthread_getspecific" => {
                let [key] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let key = this.read_scalar(key)?.check_init()?.to_bits(key.layout.size)?;
                let active_thread = this.get_active_thread();
                let ptr = this.machine.tls.load_tls(key, active_thread, this)?;
                this.write_scalar(ptr, dest)?;
            }
            "pthread_setspecific" => {
                let [key, new_ptr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let key = this.read_scalar(key)?.check_init()?.to_bits(key.layout.size)?;
                let active_thread = this.get_active_thread();
                let new_data = this.read_scalar(new_ptr)?;
                this.machine.tls.store_tls(key, active_thread, new_data.check_init()?, &*this.tcx)?;

                // Return success (`0`).
                this.write_null(dest)?;
            }

            // Synchronization primitives
            "pthread_mutexattr_init" => {
                let [attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutexattr_init(attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutexattr_settype" => {
                let [attr, kind] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutexattr_settype(attr, kind)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutexattr_destroy" => {
                let [attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutexattr_destroy(attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutex_init" => {
                let [mutex, attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutex_init(mutex, attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutex_lock" => {
                let [mutex] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutex_lock(mutex)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutex_trylock" => {
                let [mutex] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutex_trylock(mutex)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutex_unlock" => {
                let [mutex] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutex_unlock(mutex)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_mutex_destroy" => {
                let [mutex] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_mutex_destroy(mutex)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_rdlock" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_rdlock(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_tryrdlock" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_tryrdlock(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_wrlock" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_wrlock(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_trywrlock" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_trywrlock(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_unlock" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_unlock(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_rwlock_destroy" => {
                let [rwlock] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_rwlock_destroy(rwlock)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_condattr_init" => {
                let [attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_condattr_init(attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_condattr_destroy" => {
                let [attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_condattr_destroy(attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_cond_init" => {
                let [cond, attr] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_cond_init(cond, attr)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_cond_signal" => {
                let [cond] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_cond_signal(cond)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_cond_broadcast" => {
                let [cond] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_cond_broadcast(cond)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_cond_wait" => {
                let [cond, mutex] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_cond_wait(cond, mutex)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_cond_timedwait" => {
                let [cond, mutex, abstime] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.pthread_cond_timedwait(cond, mutex, abstime, dest)?;
            }
            "pthread_cond_destroy" => {
                let [cond] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_cond_destroy(cond)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }

            // Threading
            "pthread_create" => {
                let [thread, attr, start, arg] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_create(thread, attr, start, arg)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_join" => {
                let [thread, retval] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_join(thread, retval)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_detach" => {
                let [thread] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.pthread_detach(thread)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "pthread_self" => {
                let [] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.pthread_self(dest)?;
            }
            "sched_yield" => {
                let [] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.sched_yield()?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }
            "nanosleep" => {
                let [req, rem] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let result = this.nanosleep(req, rem)?;
                this.write_scalar(Scalar::from_i32(result), dest)?;
            }

            // Miscellaneous
            "isatty" => {
                let [fd] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.read_scalar(fd)?.to_i32()?;
                // "returns 1 if fd is an open file descriptor referring to a terminal; otherwise 0 is returned, and errno is set to indicate the error"
                // FIXME: we just say nothing is a terminal.
                let enotty = this.eval_libc("ENOTTY")?;
                this.set_last_error(enotty)?;
                this.write_null(dest)?;
            }
            "pthread_atfork" => {
                let [prepare, parent, child] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.read_pointer(prepare)?;
                this.read_pointer(parent)?;
                this.read_pointer(child)?;
                // We do not support forking, so there is nothing to do here.
                this.write_null(dest)?;
            }
            "strerror_r" | "__xpg_strerror_r" => {
                let [errnum, buf, buflen] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let errnum = this.read_scalar(errnum)?.check_init()?;
                let buf = this.read_pointer(buf)?;
                let buflen = this.read_scalar(buflen)?.to_machine_usize(this)?;

                let error = this.errnum_to_io_error(errnum)?;
                let formatted = error.to_string();
                let (complete, _) = this.write_os_str_to_c_str(OsStr::new(&formatted), buf, buflen)?;
                let ret = if complete { 0 } else { this.eval_libc_i32("ERANGE")? };
                this.write_int(ret, dest)?;
            }

            // Incomplete shims that we "stub out" just to get pre-main initialization code to work.
            // These shims are enabled only when the caller is in the standard library.
            "pthread_attr_getguardsize"
            if this.frame_in_std() => {
                let [_attr, guard_size] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                let guard_size = this.deref_operand(guard_size)?;
                let guard_size_layout = this.libc_ty_layout("size_t")?;
                this.write_scalar(Scalar::from_uint(crate::PAGE_SIZE, guard_size_layout.size), &guard_size.into())?;

                // Return success (`0`).
                this.write_null(dest)?;
            }

            | "pthread_attr_init"
            | "pthread_attr_destroy"
            if this.frame_in_std() => {
                let [_] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.write_null(dest)?;
            }
            | "pthread_attr_setstacksize"
            if this.frame_in_std() => {
                let [_, _] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.write_null(dest)?;
            }

            | "signal"
            | "sigaltstack"
            if this.frame_in_std() => {
                let [_, _] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.write_null(dest)?;
            }
            | "sigaction"
            | "mprotect"
            if this.frame_in_std() => {
                let [_, _, _] = this.check_shim(abi, Abi::C { unwind: false }, link_name, args)?;
                this.write_null(dest)?;
            }

            // Platform-specific shims
            _ => {
                match this.tcx.sess.target.os.as_ref() {
                    "linux" => return shims::posix::linux::foreign_items::EvalContextExt::emulate_foreign_item_by_name(this, link_name, abi, args, dest, ret),
                    "macos" => return shims::posix::macos::foreign_items::EvalContextExt::emulate_foreign_item_by_name(this, link_name, abi, args, dest, ret),
                    _ => unreachable!(),
                }
            }
        };

        Ok(EmulateByNameResult::NeedsJumping)
    }
}
