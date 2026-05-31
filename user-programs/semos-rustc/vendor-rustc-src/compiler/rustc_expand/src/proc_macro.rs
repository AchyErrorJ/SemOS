// M27 §1.5: proc-macro runtime deferred — SemOS v1 cannot dlopen a proc-macro
// dylib and has no cross-thread mpsc semantics; the proc-macro server in
// this file is therefore host-only. On the SemOS target the public API
// shape (BangProcMacro / AttrProcMacro / DeriveProcMacro / provide_derive_
// macro_expansion) is preserved as never-succeeding stubs so the rest of
// rustc_expand and the builtin_macros registration code keep type-checking.
// Resolution: once a kernel-side proc-macro sandbox lands (out-of-scope
// for M27), restore the upstream body. See PLAN §1.5 for rationale.

use rustc_ast::tokenstream::TokenStream;
use rustc_errors::ErrorGuaranteed;
use rustc_middle::ty::{self, TyCtxt};
use rustc_parse::parser::{ForceCollect, Parser};
use rustc_session::Session;
use rustc_session::config::ProcMacroExecutionStrategy;
use rustc_span::profiling::SpannedEventArgRecorder;
use rustc_span::{LocalExpnId, Span};
use {rustc_ast as ast, rustc_proc_macro as pm};

use crate::base::{self, *};
use crate::errors;
// M27 §1.5: proc_macro_server is host-only (the SemOS expand stubs never
// construct a Rustc server).
#[cfg(not(target_os = "none"))]
use crate::proc_macro_server;

// M27 §1.5: mpsc-backed MessagePipe is the cross-thread channel for the
// proc-macro server. semos_std has no mpsc shim yet; host-only.
#[cfg(not(target_os = "none"))]
struct MessagePipe<T> {
    tx: std::sync::mpsc::SyncSender<T>,
    rx: std::sync::mpsc::Receiver<T>,
}

#[cfg(not(target_os = "none"))]
impl<T> pm::bridge::server::MessagePipe<T> for MessagePipe<T> {
    fn new() -> (Self, Self) {
        let (tx1, rx1) = std::sync::mpsc::sync_channel(1);
        let (tx2, rx2) = std::sync::mpsc::sync_channel(1);
        (MessagePipe { tx: tx1, rx: rx2 }, MessagePipe { tx: tx2, rx: rx1 })
    }

    fn send(&mut self, value: T) {
        self.tx.send(value).unwrap();
    }

    fn recv(&mut self) -> Option<T> {
        self.rx.recv().ok()
    }
}

#[cfg(not(target_os = "none"))]
fn exec_strategy(sess: &Session) -> impl pm::bridge::server::ExecutionStrategy + 'static {
    pm::bridge::server::MaybeCrossThread::<MessagePipe<_>>::new(
        sess.opts.unstable_opts.proc_macro_execution_strategy
            == ProcMacroExecutionStrategy::CrossThread,
    )
}

pub struct BangProcMacro {
    pub client: pm::bridge::client::Client<pm::TokenStream, pm::TokenStream>,
}

impl base::BangProcMacro for BangProcMacro {
    #[cfg(not(target_os = "none"))]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        input: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        let _timer =
            ecx.sess.prof.generic_activity_with_arg_recorder("expand_proc_macro", |recorder| {
                recorder.record_arg_with_span(ecx.sess.source_map(), ecx.expansion_descr(), span);
            });

        let proc_macro_backtrace = ecx.ecfg.proc_macro_backtrace;
        let strategy = exec_strategy(ecx.sess);
        let server = proc_macro_server::Rustc::new(ecx);
        self.client.run(&strategy, server, input, proc_macro_backtrace).map_err(|e| {
            ecx.dcx().emit_err(errors::ProcMacroPanicked {
                span,
                message: e
                    .as_str()
                    .map(|message| errors::ProcMacroPanickedHelp { message: message.into() }),
            })
        })
    }
    // M27 §1.5: SemOS target — proc-macros are out of scope for v1. Emit a
    // hard error from the diagnostic context (the call site has Span).
    #[cfg(target_os = "none")]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        _input: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        let _ = &self.client;
        Err(ecx.dcx().emit_err(errors::ProcMacroPanicked {
            span,
            message: Some(errors::ProcMacroPanickedHelp {
                message: alloc::string::String::from(
                    "proc-macro expansion not supported by rustc-on-SemOS (PLAN \u{a7}1.5)",
                ),
            }),
        }))
    }
}

pub struct AttrProcMacro {
    pub client: pm::bridge::client::Client<(pm::TokenStream, pm::TokenStream), pm::TokenStream>,
}

impl base::AttrProcMacro for AttrProcMacro {
    #[cfg(not(target_os = "none"))]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        annotation: TokenStream,
        annotated: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        let _timer =
            ecx.sess.prof.generic_activity_with_arg_recorder("expand_proc_macro", |recorder| {
                recorder.record_arg_with_span(ecx.sess.source_map(), ecx.expansion_descr(), span);
            });

        let proc_macro_backtrace = ecx.ecfg.proc_macro_backtrace;
        let strategy = exec_strategy(ecx.sess);
        let server = proc_macro_server::Rustc::new(ecx);
        self.client.run(&strategy, server, annotation, annotated, proc_macro_backtrace).map_err(
            |e| {
                ecx.dcx().emit_err(errors::CustomAttributePanicked {
                    span,
                    message: e.as_str().map(|message| errors::CustomAttributePanickedHelp {
                        message: message.into(),
                    }),
                })
            },
        )
    }
    // M27 §1.5: SemOS-target stub — attribute proc-macros not supported.
    #[cfg(target_os = "none")]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        _annotation: TokenStream,
        _annotated: TokenStream,
    ) -> Result<TokenStream, ErrorGuaranteed> {
        let _ = &self.client;
        Err(ecx.dcx().emit_err(errors::CustomAttributePanicked {
            span,
            message: Some(errors::CustomAttributePanickedHelp {
                message: alloc::string::String::from(
                    "attribute proc-macros not supported by rustc-on-SemOS (PLAN \u{a7}1.5)",
                ),
            }),
        }))
    }
}

pub struct DeriveProcMacro {
    pub client: DeriveClient,
}

impl MultiItemModifier for DeriveProcMacro {
    #[cfg(not(target_os = "none"))]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        _meta_item: &ast::MetaItem,
        item: Annotatable,
        _is_derive_const: bool,
    ) -> ExpandResult<Vec<Annotatable>, Annotatable> {
        let _timer = ecx.sess.prof.generic_activity_with_arg_recorder(
            "expand_derive_proc_macro_outer",
            |recorder| {
                recorder.record_arg_with_span(ecx.sess.source_map(), ecx.expansion_descr(), span);
            },
        );

        // We need special handling for statement items
        // (e.g. `fn foo() { #[derive(Debug)] struct Bar; }`)
        let is_stmt = matches!(item, Annotatable::Stmt(..));

        // We used to have an alternative behaviour for crates that needed it.
        // We had a lint for a long time, but now we just emit a hard error.
        // Eventually we might remove the special case hard error check
        // altogether. See #73345.
        crate::base::ann_pretty_printing_compatibility_hack(&item, &ecx.sess.psess);
        let input = item.to_tokens();

        let invoc_id = ecx.current_expansion.id;

        let res = if ecx.sess.opts.incremental.is_some()
            && ecx.sess.opts.unstable_opts.cache_proc_macros
        {
            ty::tls::with(|tcx| {
                let input = &*tcx.arena.alloc(input);
                let key: (LocalExpnId, &TokenStream) = (invoc_id, input);

                QueryDeriveExpandCtx::enter(ecx, self.client, move || {
                    tcx.derive_macro_expansion(key).cloned()
                })
            })
        } else {
            expand_derive_macro(invoc_id, input, ecx, self.client)
        };

        let Ok(output) = res else {
            // error will already have been emitted
            return ExpandResult::Ready(vec![]);
        };

        let error_count_before = ecx.dcx().err_count();
        let mut parser = Parser::new(&ecx.sess.psess, output, Some("proc-macro derive"));
        let mut items = vec![];

        loop {
            match parser.parse_item(ForceCollect::No) {
                Ok(None) => break,
                Ok(Some(item)) => {
                    if is_stmt {
                        items.push(Annotatable::Stmt(Box::new(ecx.stmt_item(span, item))));
                    } else {
                        items.push(Annotatable::Item(item));
                    }
                }
                Err(err) => {
                    err.emit();
                    break;
                }
            }
        }

        // fail if there have been errors emitted
        if ecx.dcx().err_count() > error_count_before {
            ecx.dcx().emit_err(errors::ProcMacroDeriveTokens { span });
        }

        ExpandResult::Ready(items)
    }
    // M27 §1.5: SemOS-target stub — derive proc-macros not supported.
    #[cfg(target_os = "none")]
    fn expand(
        &self,
        ecx: &mut ExtCtxt<'_>,
        span: Span,
        _meta_item: &ast::MetaItem,
        _item: Annotatable,
        _is_derive_const: bool,
    ) -> ExpandResult<Vec<Annotatable>, Annotatable> {
        let _ = &self.client;
        ecx.dcx().emit_err(errors::ProcMacroDeriveTokens { span });
        ExpandResult::Ready(vec![])
    }
}

/// Provide a query for computing the output of a derive macro.
#[cfg(not(target_os = "none"))]
pub(super) fn provide_derive_macro_expansion<'tcx>(
    tcx: TyCtxt<'tcx>,
    key: (LocalExpnId, &'tcx TokenStream),
) -> Result<&'tcx TokenStream, ()> {
    let (invoc_id, input) = key;

    // Make sure that we invalidate the query when the crate defining the proc macro changes
    let _ = tcx.crate_hash(invoc_id.expn_data().macro_def_id.unwrap().krate);

    QueryDeriveExpandCtx::with(|ecx, client| {
        expand_derive_macro(invoc_id, input.clone(), ecx, client).map(|ts| &*tcx.arena.alloc(ts))
    })
}

// M27 §1.5: SemOS-target stub — incremental derive query path never fires.
#[cfg(target_os = "none")]
pub(super) fn provide_derive_macro_expansion<'tcx>(
    _tcx: TyCtxt<'tcx>,
    _key: (LocalExpnId, &'tcx TokenStream),
) -> Result<&'tcx TokenStream, ()> {
    Err(())
}

type DeriveClient = pm::bridge::client::Client<pm::TokenStream, pm::TokenStream>;

#[cfg(not(target_os = "none"))]
fn expand_derive_macro(
    invoc_id: LocalExpnId,
    input: TokenStream,
    ecx: &mut ExtCtxt<'_>,
    client: DeriveClient,
) -> Result<TokenStream, ()> {
    let _timer =
        ecx.sess.prof.generic_activity_with_arg_recorder("expand_proc_macro", |recorder| {
            let invoc_expn_data = invoc_id.expn_data();
            let span = invoc_expn_data.call_site;
            let event_arg = invoc_expn_data.kind.descr();
            recorder.record_arg_with_span(ecx.sess.source_map(), event_arg.clone(), span);
        });

    let proc_macro_backtrace = ecx.ecfg.proc_macro_backtrace;
    let strategy = exec_strategy(ecx.sess);
    let server = proc_macro_server::Rustc::new(ecx);

    match client.run(&strategy, server, input, proc_macro_backtrace) {
        Ok(stream) => Ok(stream),
        Err(e) => {
            let invoc_expn_data = invoc_id.expn_data();
            let span = invoc_expn_data.call_site;
            ecx.dcx().emit_err({
                errors::ProcMacroDerivePanicked {
                    span,
                    message: e.as_str().map(|message| errors::ProcMacroDerivePanickedHelp {
                        message: message.into(),
                    }),
                }
            });
            Err(())
        }
    }
}

/// Stores the context necessary to expand a derive proc macro via a query.
#[cfg(not(target_os = "none"))]
struct QueryDeriveExpandCtx {
    /// Type-erased version of `&mut ExtCtxt`
    expansion_ctx: *mut (),
    client: DeriveClient,
}

#[cfg(not(target_os = "none"))]
impl QueryDeriveExpandCtx {
    /// Store the extension context and the client into the thread local value.
    /// It will be accessible via the `with` method while `f` is active.
    fn enter<F, R>(ecx: &mut ExtCtxt<'_>, client: DeriveClient, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // We need erasure to get rid of the lifetime
        let ctx = Self { expansion_ctx: ecx as *mut _ as *mut (), client };
        DERIVE_EXPAND_CTX.set(&ctx, || f())
    }

    /// Accesses the thread local value of the derive expansion context.
    /// Must be called while the `enter` function is active.
    fn with<F, R>(f: F) -> R
    where
        F: for<'a, 'b> FnOnce(&'b mut ExtCtxt<'a>, DeriveClient) -> R,
    {
        DERIVE_EXPAND_CTX.with(|ctx| {
            let ectx = {
                let casted = ctx.expansion_ctx.cast::<ExtCtxt<'_>>();
                // SAFETY: We can only get the value from `with` while the `enter` function
                // is active (on the callstack), and that function's signature ensures that the
                // lifetime is valid.
                // If `with` is called at some other time, it will panic due to usage of
                // `scoped_tls::with`.
                unsafe { casted.as_mut().unwrap() }
            };

            f(ectx, ctx.client)
        })
    }
}

// When we invoke a query to expand a derive proc macro, we need to provide it with the expansion
// context and derive Client. We do that using a thread-local.
// M27 R4 B2: scoped_tls — see semos_std::scoped_thread_local! shim. Kept
// behind cfg(not(target_os = "none")) because the QueryDeriveExpandCtx
// only exists on host (proc-macro path).
#[cfg(not(target_os = "none"))]
scoped_tls::scoped_thread_local!(static DERIVE_EXPAND_CTX: QueryDeriveExpandCtx);
