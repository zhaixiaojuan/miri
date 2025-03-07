//! Panic runtime for Miri.
//!
//! The core pieces of the runtime are:
//! - An implementation of `__rust_maybe_catch_panic` that pushes the invoked stack frame with
//!   some extra metadata derived from the panic-catching arguments of `__rust_maybe_catch_panic`.
//! - A hack in `libpanic_unwind` that calls the `miri_start_panic` intrinsic instead of the
//!   target-native panic runtime. (This lives in the rustc repo.)
//! - An implementation of `miri_start_panic` that stores its argument (the panic payload), and then
//!   immediately returns, but on the *unwind* edge (not the normal return edge), thus initiating unwinding.
//! - A hook executed each time a frame is popped, such that if the frame pushed by `__rust_maybe_catch_panic`
//!   gets popped *during unwinding*, we take the panic payload and store it according to the extra
//!   metadata we remembered when pushing said frame.

use log::trace;

use rustc_ast::Mutability;
use rustc_middle::{mir, ty};
use rustc_span::Symbol;
use rustc_target::spec::abi::Abi;
use rustc_target::spec::PanicStrategy;

use crate::*;
use helpers::check_arg_count;

/// Holds all of the relevant data for when unwinding hits a `try` frame.
#[derive(Debug)]
pub struct CatchUnwindData<'tcx> {
    /// The `catch_fn` callback to call in case of a panic.
    catch_fn: Scalar<Tag>,
    /// The `data` argument for that callback.
    data: Scalar<Tag>,
    /// The return place from the original call to `try`.
    dest: PlaceTy<'tcx, Tag>,
    /// The return block from the original call to `try`.
    ret: mir::BasicBlock,
}

impl<'mir, 'tcx: 'mir> EvalContextExt<'mir, 'tcx> for crate::MiriEvalContext<'mir, 'tcx> {}
pub trait EvalContextExt<'mir, 'tcx: 'mir>: crate::MiriEvalContextExt<'mir, 'tcx> {
    /// Handles the special `miri_start_panic` intrinsic, which is called
    /// by libpanic_unwind to delegate the actual unwinding process to Miri.
    fn handle_miri_start_panic(
        &mut self,
        abi: Abi,
        link_name: Symbol,
        args: &[OpTy<'tcx, Tag>],
        unwind: StackPopUnwind,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        trace!("miri_start_panic: {:?}", this.frame().instance);

        // Get the raw pointer stored in arg[0] (the panic payload).
        let [payload] = this.check_shim(abi, Abi::Rust, link_name, args)?;
        let payload = this.read_scalar(payload)?.check_init()?;
        let thread = this.active_thread_mut();
        assert!(thread.panic_payload.is_none(), "the panic runtime should avoid double-panics");
        thread.panic_payload = Some(payload);

        // Jump to the unwind block to begin unwinding.
        this.unwind_to_block(unwind)?;
        Ok(())
    }

    /// Handles the `try` intrinsic, the underlying implementation of `std::panicking::try`.
    fn handle_try(
        &mut self,
        args: &[OpTy<'tcx, Tag>],
        dest: &PlaceTy<'tcx, Tag>,
        ret: mir::BasicBlock,
    ) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        // Signature:
        //   fn r#try(try_fn: fn(*mut u8), data: *mut u8, catch_fn: fn(*mut u8, *mut u8)) -> i32
        // Calls `try_fn` with `data` as argument. If that executes normally, returns 0.
        // If that unwinds, calls `catch_fn` with the first argument being `data` and
        // then second argument being a target-dependent `payload` (i.e. it is up to us to define
        // what that is), and returns 1.
        // The `payload` is passed (by libstd) to `__rust_panic_cleanup`, which is then expected to
        // return a `Box<dyn Any + Send + 'static>`.
        // In Miri, `miri_start_panic` is passed exactly that type, so we make the `payload` simply
        // a pointer to `Box<dyn Any + Send + 'static>`.

        // Get all the arguments.
        let [try_fn, data, catch_fn] = check_arg_count(args)?;
        let try_fn = this.read_pointer(try_fn)?;
        let data = this.read_scalar(data)?.check_init()?;
        let catch_fn = this.read_scalar(catch_fn)?.check_init()?;

        // Now we make a function call, and pass `data` as first and only argument.
        let f_instance = this.get_ptr_fn(try_fn)?.as_instance()?;
        trace!("try_fn: {:?}", f_instance);
        let ret_place = MPlaceTy::dangling(this.machine.layouts.unit).into();
        this.call_function(
            f_instance,
            Abi::Rust,
            &[data.into()],
            Some(&ret_place),
            // Directly return to caller.
            StackPopCleanup::Goto { ret: Some(ret), unwind: StackPopUnwind::Skip },
        )?;

        // We ourselves will return `0`, eventually (will be overwritten if we catch a panic).
        this.write_null(dest)?;

        // In unwind mode, we tag this frame with the extra data needed to catch unwinding.
        // This lets `handle_stack_pop` (below) know that we should stop unwinding
        // when we pop this frame.
        if this.tcx.sess.panic_strategy() == PanicStrategy::Unwind {
            this.frame_mut().extra.catch_unwind =
                Some(CatchUnwindData { catch_fn, data, dest: *dest, ret });
        }

        Ok(())
    }

    fn handle_stack_pop(
        &mut self,
        mut extra: FrameData<'tcx>,
        unwinding: bool,
    ) -> InterpResult<'tcx, StackPopJump> {
        let this = self.eval_context_mut();

        trace!("handle_stack_pop(extra = {:?}, unwinding = {})", extra, unwinding);
        if let Some(stacked_borrows) = &this.machine.stacked_borrows {
            stacked_borrows.borrow_mut().end_call(extra.call_id);
        }

        // We only care about `catch_panic` if we're unwinding - if we're doing a normal
        // return, then we don't need to do anything special.
        if let (true, Some(catch_unwind)) = (unwinding, extra.catch_unwind.take()) {
            // We've just popped a frame that was pushed by `try`,
            // and we are unwinding, so we should catch that.
            trace!(
                "unwinding: found catch_panic frame during unwinding: {:?}",
                this.frame().instance
            );

            // We set the return value of `try` to 1, since there was a panic.
            this.write_scalar(Scalar::from_i32(1), &catch_unwind.dest)?;

            // The Thread's `panic_payload` holds what was passed to `miri_start_panic`.
            // This is exactly the second argument we need to pass to `catch_fn`.
            let payload = this.active_thread_mut().panic_payload.take().unwrap();

            // Push the `catch_fn` stackframe.
            let f_instance =
                this.get_ptr_fn(this.scalar_to_ptr(catch_unwind.catch_fn)?)?.as_instance()?;
            trace!("catch_fn: {:?}", f_instance);
            let ret_place = MPlaceTy::dangling(this.machine.layouts.unit).into();
            this.call_function(
                f_instance,
                Abi::Rust,
                &[catch_unwind.data.into(), payload.into()],
                Some(&ret_place),
                // Directly return to caller of `try`.
                StackPopCleanup::Goto { ret: Some(catch_unwind.ret), unwind: StackPopUnwind::Skip },
            )?;

            // We pushed a new stack frame, the engine should not do any jumping now!
            Ok(StackPopJump::NoJump)
        } else {
            Ok(StackPopJump::Normal)
        }
    }

    /// Start a panic in the interpreter with the given message as payload.
    fn start_panic(&mut self, msg: &str, unwind: StackPopUnwind) -> InterpResult<'tcx> {
        let this = self.eval_context_mut();

        // First arg: message.
        let msg = this.allocate_str(msg, MiriMemoryKind::Machine.into(), Mutability::Not);

        // Call the lang item.
        let panic = this.tcx.lang_items().panic_fn().unwrap();
        let panic = ty::Instance::mono(this.tcx.tcx, panic);
        this.call_function(
            panic,
            Abi::Rust,
            &[msg.to_ref(this)],
            None,
            StackPopCleanup::Goto { ret: None, unwind },
        )
    }

    fn assert_panic(
        &mut self,
        msg: &mir::AssertMessage<'tcx>,
        unwind: Option<mir::BasicBlock>,
    ) -> InterpResult<'tcx> {
        use rustc_middle::mir::AssertKind::*;
        let this = self.eval_context_mut();

        match msg {
            BoundsCheck { index, len } => {
                // Forward to `panic_bounds_check` lang item.

                // First arg: index.
                let index = this.read_scalar(&this.eval_operand(index, None)?)?;
                // Second arg: len.
                let len = this.read_scalar(&this.eval_operand(len, None)?)?;

                // Call the lang item.
                let panic_bounds_check = this.tcx.lang_items().panic_bounds_check_fn().unwrap();
                let panic_bounds_check = ty::Instance::mono(this.tcx.tcx, panic_bounds_check);
                this.call_function(
                    panic_bounds_check,
                    Abi::Rust,
                    &[index.into(), len.into()],
                    None,
                    StackPopCleanup::Goto {
                        ret: None,
                        unwind: match unwind {
                            Some(cleanup) => StackPopUnwind::Cleanup(cleanup),
                            None => StackPopUnwind::Skip,
                        },
                    },
                )?;
            }
            _ => {
                // Forward everything else to `panic` lang item.
                this.start_panic(
                    msg.description(),
                    match unwind {
                        Some(cleanup) => StackPopUnwind::Cleanup(cleanup),
                        None => StackPopUnwind::Skip,
                    },
                )?;
            }
        }
        Ok(())
    }
}
