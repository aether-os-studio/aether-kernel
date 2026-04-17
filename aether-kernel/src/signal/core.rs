use super::abi::{
    SIGABRT, SIGALRM, SIGBUS, SIGCHLD, SIGCONT, SIGFPE, SIGHUP, SIGILL, SIGINT, SIGIO, SIGKILL,
    SIGPIPE, SIGPROF, SIGPWR, SIGQUIT, SIGSEGV, SIGSTOP, SIGSYS, SIGTERM, SIGTRAP, SIGTSTP,
    SIGTTIN, SIGTTOU, SIGURG, SIGUSR1, SIGUSR2, SIGVTALRM, SIGWINCH, SIGXCPU, SIGXFSZ, SigSet,
    SignalAction, SignalInfo, sigbit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDefault {
    Ignore,
    Terminate,
    Stop,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDelivery {
    None,
    Ignored(SignalInfo),
    Deliver(SignalInfo, SignalAction),
    Exit(SignalInfo),
    Stop(SignalInfo),
    Continue(SignalInfo),
}

pub fn default_action(signal: u8) -> SignalDefault {
    match signal {
        SIGCHLD | SIGURG | SIGWINCH => SignalDefault::Ignore,
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => SignalDefault::Stop,
        SIGCONT => SignalDefault::Continue,
        SIGHUP | SIGINT | SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGBUS | SIGFPE | SIGKILL
        | SIGUSR1 | SIGSEGV | SIGUSR2 | SIGPIPE | SIGALRM | SIGTERM | SIGXCPU | SIGXFSZ
        | SIGVTALRM | SIGPROF | SIGIO | SIGPWR | SIGSYS => SignalDefault::Terminate,
        _ => SignalDefault::Terminate,
    }
}

pub fn sanitize_mask(mask: SigSet) -> SigSet {
    mask & !(sigbit(SIGKILL) | sigbit(SIGSTOP))
}

pub fn is_blocked(mask: SigSet, signal: u8) -> bool {
    (mask & sigbit(signal)) != 0 && signal != SIGKILL && signal != SIGSTOP
}

pub fn delivery_for(
    blocked: SigSet,
    action: SignalAction,
    info: SignalInfo,
    handlers_supported: bool,
) -> SignalDelivery {
    if is_blocked(blocked, info.signal) {
        return SignalDelivery::None;
    }

    if action.handler == super::abi::SIG_IGN {
        return SignalDelivery::Ignored(info);
    }

    if action.handler != super::abi::SIG_DFL {
        return if handlers_supported {
            SignalDelivery::Deliver(info, action)
        } else {
            SignalDelivery::Exit(info)
        };
    }

    match default_action(info.signal) {
        SignalDefault::Ignore => SignalDelivery::Ignored(info),
        SignalDefault::Terminate => SignalDelivery::Exit(info),
        SignalDefault::Stop => SignalDelivery::Stop(info),
        SignalDefault::Continue => SignalDelivery::Continue(info),
    }
}
