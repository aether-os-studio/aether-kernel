extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use aether_device::{DeviceRegistry, KernelDevice};
use aether_drivers::{DriverInventory, register_input_sink};
use aether_frame::boot;
use aether_frame::libs::spin::{LocalIrqDisabled, SpinLock};
use aether_frame::logger::RegisterWriterError;
use aether_frame::time;
use aether_framebuffer::{FramebufferDevice, FramebufferError, FramebufferSurface, RgbColor};
use aether_kmsg::KmsgBuffer;
use aether_process::{BuildError, ElfProgramBuilder, UserProgramBuilder, elf_interpreter_path};
use aether_terminal::{FramebufferConsole, register_process_group_signal_hook};
use aether_vfs::{FileNode, FsError, NodeRef, OpenFlags};
use spin::Once;

use crate::arch::ArchContext;
use crate::errno::{SysErr, SysResult};
use crate::fs::{FdTable, NodeImageSource, PidFdHandle, create_pidfd_node};
use crate::log_sinks;
use crate::process::{
    ChildEvent, CloneParams, DispatchWork, KernelProcess, Pid, ProcFsProcessSnapshot,
    ProcessManager, ProcessServices, ProcessState, ScheduleEvent, anonymous_filesystem_identity,
};
use crate::rootfs::{ExecFormat, ProcessFsContext, RootfsError, RootfsManager};
use crate::syscall::SyscallArgs;

#[derive(Debug)]
pub enum RuntimeInitError {
    FileSystem(FsError),
    Framebuffer(FramebufferError),
    LogWriter(RegisterWriterError),
    Processor(&'static str),
    Process(BuildError),
    Rootfs(RootfsError),
}

impl From<FsError> for RuntimeInitError {
    fn from(value: FsError) -> Self {
        Self::FileSystem(value)
    }
}

impl From<FramebufferError> for RuntimeInitError {
    fn from(value: FramebufferError) -> Self {
        Self::Framebuffer(value)
    }
}

impl From<RegisterWriterError> for RuntimeInitError {
    fn from(value: RegisterWriterError) -> Self {
        Self::LogWriter(value)
    }
}

impl From<BuildError> for RuntimeInitError {
    fn from(value: BuildError) -> Self {
        Self::Process(value)
    }
}

impl From<RootfsError> for RuntimeInitError {
    fn from(value: RootfsError) -> Self {
        Self::Rootfs(value)
    }
}

pub struct KernelRuntime {
    rootfs: Arc<RootfsManager>,
    _devices: DeviceRegistry,
    _drivers: DriverInventory,
    _kmsg: Arc<KmsgBuffer>,
    _console: Option<Arc<FramebufferConsole>>,
    processes: SpinLock<ProcessManager>,
}

static SHARED_RUNTIME: Once<Arc<KernelRuntime>> = Once::new();
static TIMER_TICK_PENDING: AtomicBool = AtomicBool::new(false);
static DRM_VBLANK_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT_TIMER_DEADLINE_NANOS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);
static PROCESS_GROUP_SIGNAL_PENDING: AtomicBool = AtomicBool::new(false);
static PENDING_PROCESS_GROUP_SIGNALS: SpinLock<VecDeque<(i32, i32)>, LocalIrqDisabled> =
    SpinLock::new(VecDeque::new());

const PRECISE_WAIT_THRESHOLD_NS: u64 = 10_000_000;
const IDLE_POLL_INTERVAL_NS: u64 = 1_000_000;

pub(crate) fn publish_next_timer_deadline(deadline_nanos: Option<u64>) {
    NEXT_TIMER_DEADLINE_NANOS.store(deadline_nanos.unwrap_or(u64::MAX), Ordering::Release);
}

fn runtime_tick_handler() {
    if aether_frame::arch::cpu::current_cpu_index() == 0
        && aether_drivers::drm::vblank_deadline_due()
    {
        DRM_VBLANK_PENDING.store(true, Ordering::Release);
    }
    let deadline_nanos = NEXT_TIMER_DEADLINE_NANOS.load(Ordering::Acquire);
    let process_due = deadline_nanos != u64::MAX && time::monotonic_nanos() >= deadline_nanos;
    if process_due || crate::fs::timerfd_deadline_due() {
        TIMER_TICK_PENDING.store(true, Ordering::Release);
    }
}

fn runtime_send_process_group_signal(process_group: i32, signal: i32) {
    if process_group <= 0 || signal <= 0 {
        return;
    }
    PENDING_PROCESS_GROUP_SIGNALS
        .lock()
        .push_back((process_group, signal));
    PROCESS_GROUP_SIGNAL_PENDING.store(true, Ordering::Release);
    aether_frame::preempt::request_reschedule();
}

impl KernelRuntime {
    fn drain_pending_runtime_events(&self) {
        let current_nanos = time::monotonic_nanos();
        if aether_frame::arch::cpu::current_cpu_index() == 0
            && (DRM_VBLANK_PENDING.swap(false, Ordering::AcqRel)
                || aether_drivers::drm::next_vblank_wakeup_deadline().is_some())
        {
            aether_drivers::drm::handle_vblank_tick();
        }

        let timer_tick_pending = TIMER_TICK_PENDING.swap(false, Ordering::AcqRel);
        let mut processes = self.processes.lock();

        if timer_tick_pending
            || crate::fs::next_timerfd_wakeup_deadline().is_some()
            || processes.next_timer_deadline_nanos().is_some()
        {
            crate::fs::wake_expired_timerfds();
            if processes.timer_deadline_due(current_nanos) {
                processes.wake_expired_timers(current_nanos);
            }
        }

        processes.wake_ready_file_blocks();
    }

    pub fn current_pid() -> Option<Pid> {
        crate::processor::current_pid()
    }

    pub fn procfs_snapshot(pid: Pid) -> Option<ProcFsProcessSnapshot> {
        Self::shared().and_then(|runtime| runtime.processes.lock().procfs_snapshot(pid))
    }

    pub fn bootstrap() -> Result<Self, RuntimeInitError> {
        crate::processor::init_current_cpu().map_err(RuntimeInitError::Processor)?;
        log_sinks::init()?;
        crate::syscall::init();
        crate::net::init();

        let rootfs_manager = RootfsManager::new()?;
        let rootfs = Arc::new(rootfs_manager);
        if let Err(error) = rootfs.register_pci_bus() {
            log::warn!("failed to register pci sysfs bus: {:?}", error);
        }

        let kmsg = Arc::new(KmsgBuffer::default());
        log_sinks::install_kmsg(kmsg.clone());
        rootfs.device_namespace().install(
            rootfs.vfs(),
            "kmsg",
            FileNode::new_char_device("kmsg", 1, 11, kmsg.file()),
        )?;

        let mut devices = DeviceRegistry::new();
        let mut console = None;

        for device in crate::devices::builtin_devices() {
            devices.register(device);
        }

        if let Some(info) = boot::info().framebuffer {
            let surface = FramebufferSurface::from_boot_info(info)?;
            surface.clear(RgbColor::BLACK);

            let fbdev: Arc<dyn KernelDevice> = Arc::new(FramebufferDevice::primary(surface));
            devices.register(fbdev);

            let terminal = Arc::new(FramebufferConsole::new(surface));
            log_sinks::install_terminal(terminal.clone());
            let tty: Arc<dyn KernelDevice> = terminal.clone();
            devices.register(tty);
            let input_sink: Arc<dyn aether_drivers::InputEventSink> = terminal.clone();
            register_input_sink(input_sink);
            console = Some(terminal);
        } else {
            log::warn!("bootloader did not provide a framebuffer");
        }

        register_process_group_signal_hook(runtime_send_process_group_signal);

        let drivers = aether_drivers::probe_all(&mut devices);

        for device in devices.devices() {
            if let Err(error) = rootfs.register_device(device.clone()) {
                log::warn!(
                    "failed to register kernel device {}: {:?}",
                    device.metadata().name,
                    error
                );
            }
        }

        let mut processes = ProcessManager::new();
        let initial_fs = rootfs.initial_fs_context();
        processes.set_initial_fs(initial_fs.fork_copy());
        processes.set_initial_files(build_initial_files(rootfs.as_ref())?);
        let init_plan = rootfs.prepare_exec(
            &initial_fs,
            "/init",
            vec![String::from("/init")],
            vec![String::from("PATH=/bin:/sbin:/usr/bin:/usr/sbin")],
        );

        match init_plan {
            Ok(plan) => match plan.format {
                ExecFormat::Flat => {
                    let argv_refs = plan.argv.iter().map(String::as_str).collect::<Vec<_>>();
                    let envp_refs = plan.envp.iter().map(String::as_str).collect::<Vec<_>>();
                    let source = NodeImageSource::new(plan.node.clone())
                        .ok_or(RuntimeInitError::Process(BuildError::EmptyProgram))?;
                    let task = UserProgramBuilder::new(&source)
                        .argv(&argv_refs)
                        .envp(&envp_refs)
                        .execfn(&plan.exec_path)
                        .build()?;
                    let init_pid = processes.spawn_task(&plan.exec_path, None, task)?;
                    log::info!(
                        "runtime: spawned init pid {} from {}",
                        init_pid,
                        plan.requested_path
                    );
                }
                ExecFormat::Elf => {
                    let argv_refs = plan.argv.iter().map(String::as_str).collect::<Vec<_>>();
                    let envp_refs = plan.envp.iter().map(String::as_str).collect::<Vec<_>>();
                    let executable = NodeImageSource::new(plan.node.clone())
                        .ok_or(RuntimeInitError::Process(BuildError::InvalidElf))?;
                    let interpreter = elf_interpreter_path(&executable)
                        .map_err(|_| RuntimeInitError::Process(BuildError::InvalidElf))?
                        .map(|path| {
                            let node = rootfs
                                .lookup_in(&initial_fs, path.as_str(), true)
                                .map_err(|_| RuntimeInitError::Process(BuildError::InvalidElf))?;
                            NodeImageSource::new(node)
                                .ok_or(RuntimeInitError::Process(BuildError::InvalidElf))
                        })
                        .transpose()?;

                    let mut builder = ElfProgramBuilder::new(&executable)
                        .argv(&argv_refs)
                        .envp(&envp_refs)
                        .execfn(&plan.exec_path);
                    if let Some(ref interpreter) = interpreter {
                        builder = builder.interpreter(interpreter);
                    }
                    let task = builder.build()?;

                    let init_pid = processes.spawn_task(&plan.exec_path, None, task)?;
                    log::info!(
                        "runtime: spawned ELF init pid {} from {}",
                        init_pid,
                        plan.requested_path
                    );
                }
            },
            Err(error) => {
                log::warn!(
                    "runtime: failed to prepare /init from initramfs: {:?}",
                    error
                );
            }
        }

        Ok(Self {
            rootfs,
            _devices: devices,
            _drivers: drivers,
            _kmsg: kmsg,
            _console: console,
            processes: SpinLock::new(processes),
        })
    }

    pub fn install(self) -> Arc<Self> {
        let runtime = Arc::new(self);
        let _ = SHARED_RUNTIME.call_once(|| runtime.clone());
        aether_frame::interrupt::timer::register_tick_handler(runtime_tick_handler);
        runtime
    }

    pub fn shared() -> Option<Arc<Self>> {
        SHARED_RUNTIME.get().cloned()
    }

    pub fn run_secondary(cpu_index: usize) -> ! {
        loop {
            if let Some(runtime) = Self::shared() {
                log::info!("secondary cpu {} entering shared scheduler", cpu_index);
                runtime.run_on_cpu(cpu_index);
            }
            aether_frame::arch::cpu::wait_for_interrupt();
        }
    }

    pub fn run_on_cpu(&self, _cpu_index: usize) -> ! {
        crate::processor::init_current_cpu().expect("failed to initialize cpu-local processor");
        loop {
            if !aether_frame::interrupt::are_enabled() {
                aether_frame::interrupt::enable();
            }
            aether_async::run_ready();
            self.drain_pending_process_group_signals();
            self.drain_pending_runtime_events();
            if aether_frame::preempt::take_need_resched() {
                continue;
            }
            let work = {
                if let Some(work) = {
                    let mut processes = self.processes.lock();
                    crate::processor::try_take_current_cpu_work(&mut processes)
                } {
                    work
                } else {
                    {
                        let mut processes = self.processes.lock();
                        if let Some(work) =
                            crate::processor::try_take_current_cpu_work(&mut processes)
                        {
                            work
                        } else {
                            DispatchWork::Idle
                        }
                    }
                }
            };
            match work {
                DispatchWork::Idle => {
                    if aether_frame::preempt::take_need_resched() {
                        continue;
                    }

                    let next_deadline = {
                        let processes = self.processes.lock();
                        let process_deadline = processes.next_timer_deadline_nanos();
                        let timerfd_deadline = crate::fs::next_timerfd_wakeup_deadline();
                        let drm_deadline = aether_drivers::drm::next_vblank_wakeup_deadline();
                        [process_deadline, timerfd_deadline, drm_deadline]
                            .into_iter()
                            .flatten()
                            .min()
                    };

                    if let Some(deadline) = next_deadline {
                        let now = time::monotonic_nanos();
                        let remaining = deadline.saturating_sub(now);
                        if remaining != 0
                            && remaining <= PRECISE_WAIT_THRESHOLD_NS
                            && time::spin_delay_nanos(remaining).is_ok()
                        {
                            if aether_drivers::drm::vblank_deadline_due() {
                                DRM_VBLANK_PENDING.store(true, Ordering::Release);
                            }
                            let current_nanos = time::monotonic_nanos();
                            let process_timer_due = {
                                let processes = self.processes.lock();
                                processes.timer_deadline_due(current_nanos)
                            };
                            if crate::fs::timerfd_deadline_due() || process_timer_due {
                                TIMER_TICK_PENDING.store(true, Ordering::Release);
                            }
                            continue;
                        }
                    }

                    let wait_ns = next_deadline
                        .map(|deadline| deadline.saturating_sub(time::monotonic_nanos()))
                        .map(|remaining| remaining.min(IDLE_POLL_INTERVAL_NS))
                        .unwrap_or(IDLE_POLL_INTERVAL_NS);
                    if wait_ns != 0 && time::spin_delay_nanos(wait_ns).is_ok() {
                        continue;
                    }
                    aether_frame::arch::cpu::wait_for_interrupt();
                }
                DispatchWork::Event(event) => self.handle_event(event),
                DispatchWork::Process(mut process) => {
                    let event =
                        crate::processor::with_current_process(process.running_snapshot(), || {
                            let result = process.task.process.run();
                            match result.reason {
                                aether_frame::process::RunReason::Syscall => {
                                    let syscall_number = result.context.syscall_number();
                                    let syscall_args = SyscallArgs::from_context(&result.context);
                                    process.pending_syscall =
                                        Some(crate::process::PendingSyscall {
                                            number: syscall_number,
                                            args: syscall_args,
                                        });
                                    process.pending_syscall_name = "";
                                    let services = RuntimeServices {
                                        rootfs: &self.rootfs,
                                        processes: &self.processes,
                                    };
                                    let dispatch = ProcessManager::dispatch_pending_syscall(
                                        &mut process,
                                        services,
                                    );
                                    let mut processes = self.processes.lock();
                                    processes.finish_syscall_dispatch(
                                        process,
                                        syscall_number,
                                        dispatch,
                                    )
                                }
                                _ => {
                                    let services = RuntimeServices {
                                        rootfs: &self.rootfs,
                                        processes: &self.processes,
                                    };
                                    let mut processes = self.processes.lock();
                                    processes.finish_process(process, result, services)
                                }
                            }
                        });
                    self.handle_event(event);
                }
                DispatchWork::KernelSyscall(process) => {
                    let event =
                        crate::processor::with_current_process(process.running_snapshot(), || {
                            let pending = process
                                .pending_syscall
                                .expect("resumed syscall missing pending state");
                            let services = RuntimeServices {
                                rootfs: &self.rootfs,
                                processes: &self.processes,
                            };
                            let mut process = process;
                            let dispatch =
                                ProcessManager::dispatch_pending_syscall(&mut process, services);
                            let mut processes = self.processes.lock();
                            processes.finish_syscall_dispatch(process, pending.number, dispatch)
                        });
                    self.handle_event(event);
                }
            }
        }
    }

    fn handle_event(&self, event: ScheduleEvent) {
        match event {
            ScheduleEvent::Idle => {
                if !aether_frame::preempt::take_need_resched() {
                    aether_frame::arch::cpu::wait_for_interrupt();
                }
            }
            ScheduleEvent::Syscall { .. } => {}
            ScheduleEvent::Interrupted { .. } => {}
            ScheduleEvent::Exited { .. } => {}
            ScheduleEvent::Faulted {
                pid,
                vector,
                error_code,
            } => {
                log::error!(
                    "process {} faulted: vector={} error_code={:#x}",
                    pid,
                    vector,
                    error_code,
                );
            }
        }
    }

    fn drain_pending_process_group_signals(&self) {
        if !PROCESS_GROUP_SIGNAL_PENDING.swap(false, Ordering::AcqRel) {
            return;
        }

        let mut signals = Vec::new();
        {
            let mut pending = PENDING_PROCESS_GROUP_SIGNALS.lock();
            while let Some(signal) = pending.pop_front() {
                signals.push(signal);
            }
        }

        if signals.is_empty() {
            return;
        }

        let mut processes = self.processes.lock();
        for (process_group, signal) in signals {
            let info = crate::signal::SignalInfo::kernel(signal as u8, 0);
            let _ = processes.send_signal_process_group(process_group as u32, info);
        }
    }
}

struct RuntimeServices<'a> {
    rootfs: &'a Arc<RootfsManager>,
    processes: &'a SpinLock<ProcessManager>,
}

impl ProcessServices for RuntimeServices<'_> {
    fn lookup_node_with_identity(
        &mut self,
        fs: &ProcessFsContext,
        path: &str,
        follow_final: bool,
    ) -> SysResult<(NodeRef, crate::fs::FileSystemIdentity)> {
        self.rootfs.lookup_in_with_identity(fs, path, follow_final)
    }

    fn statfs(&mut self, fs: &ProcessFsContext, path: &str) -> SysResult<crate::fs::LinuxStatFs> {
        self.rootfs.statfs_in(fs, path)
    }

    fn mkdir(&mut self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64> {
        self.rootfs.mkdir_in(fs, path, mode)
    }

    fn create_file(
        &mut self,
        fs: &ProcessFsContext,
        path: &str,
        mode: u32,
    ) -> SysResult<(NodeRef, crate::fs::FileSystemIdentity)> {
        self.rootfs.create_file_in(fs, path, mode)
    }

    fn create_symlink(
        &mut self,
        fs: &ProcessFsContext,
        path: &str,
        target: &str,
    ) -> SysResult<u64> {
        self.rootfs.create_symlink_in(fs, path, target)
    }

    fn bind_socket(&mut self, fs: &ProcessFsContext, path: &str, mode: u32) -> SysResult<u64> {
        self.rootfs.bind_socket_in(fs, path, mode)
    }

    fn unlink(&mut self, fs: &ProcessFsContext, path: &str, flags: u64) -> SysResult<u64> {
        self.rootfs.unlink_in(fs, path, flags)
    }

    fn link(
        &mut self,
        fs: &ProcessFsContext,
        old_path: &str,
        new_path: &str,
        flags: u64,
    ) -> SysResult<u64> {
        self.rootfs.link_in(fs, old_path, new_path, flags)
    }

    fn rename(&mut self, fs: &ProcessFsContext, old_path: &str, new_path: &str) -> SysResult<u64> {
        self.rootfs.rename_in(fs, old_path, new_path)
    }

    fn getcwd(&mut self, fs: &ProcessFsContext) -> String {
        self.rootfs.getcwd_in(fs)
    }

    fn chdir(&mut self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64> {
        self.rootfs.chdir_in(fs, path)
    }

    fn chroot(&mut self, fs: &mut ProcessFsContext, path: &str) -> SysResult<u64> {
        self.rootfs.chroot_in(fs, path)
    }

    fn mount(
        &mut self,
        fs: &mut ProcessFsContext,
        source: Option<&str>,
        target: &str,
        fstype: Option<&str>,
        flags: u64,
    ) -> SysResult<u64> {
        self.rootfs.mount_in(fs, source, target, fstype, flags)
    }

    fn umount(&mut self, fs: &ProcessFsContext, target: &str, flags: u64) -> SysResult<u64> {
        self.rootfs.umount_in(fs, target, flags)
    }

    fn pivot_root(
        &mut self,
        fs: &mut ProcessFsContext,
        new_root: &str,
        put_old: &str,
    ) -> SysResult<u64> {
        self.rootfs.pivot_root_in(fs, new_root, put_old)
    }

    fn execve(
        &mut self,
        process: &mut KernelProcess,
        path: &str,
        argv: Vec<String>,
        envp: Vec<String>,
    ) -> SysResult<u64> {
        let argv = if argv.is_empty() {
            vec![String::from(path)]
        } else {
            argv
        };
        let plan = self.rootfs.prepare_exec(&process.fs, path, argv, envp)?;

        match plan.format {
            ExecFormat::Flat => {
                let argv_refs = plan.argv.iter().map(String::as_str).collect::<Vec<_>>();
                let envp_refs = plan.envp.iter().map(String::as_str).collect::<Vec<_>>();
                let source = NodeImageSource::new(plan.node.clone()).ok_or(SysErr::Inval)?;
                let new_task = UserProgramBuilder::new(&source)
                    .argv(&argv_refs)
                    .envp(&envp_refs)
                    .execfn(&plan.exec_path)
                    .build()
                    .map_err(|_| SysErr::Inval)?;

                process.prctl.set_name(&plan.exec_path);
                process.pending_exec = Some(new_task);
                Ok(0)
            }
            ExecFormat::Elf => {
                let argv_refs = plan.argv.iter().map(String::as_str).collect::<Vec<_>>();
                let envp_refs = plan.envp.iter().map(String::as_str).collect::<Vec<_>>();
                let executable = NodeImageSource::new(plan.node.clone()).ok_or(SysErr::Inval)?;
                let interpreter = elf_interpreter_path(&executable)
                    .map_err(|_| SysErr::Inval)?
                    .map(|path| {
                        let node = self.rootfs.lookup_in(&process.fs, path.as_str(), true)?;
                        NodeImageSource::new(node).ok_or(SysErr::Inval)
                    })
                    .transpose()?;

                let new_task = {
                    let mut builder = ElfProgramBuilder::new(&executable)
                        .argv(&argv_refs)
                        .envp(&envp_refs)
                        .execfn(&plan.exec_path);
                    if let Some(ref interpreter) = interpreter {
                        builder = builder.interpreter(interpreter);
                    }
                    builder.build().map_err(|error| {
                        log::error!(
                            "execve build failed: pid={} path={} error={:?}",
                            process.identity.pid,
                            plan.exec_path,
                            error
                        );
                        SysErr::Inval
                    })?
                };

                process.prctl.set_name(&plan.exec_path);
                process.pending_exec = Some(new_task);
                Ok(0)
            }
        }
    }

    fn clone_process(&mut self, parent: &mut KernelProcess, params: CloneParams) -> SysResult<Pid> {
        params.validate()?;
        let _ = params.share_sysvsem();
        let pid = self.processes.lock().allocate_pid(params.requested_pid)?;
        let pidfd = PidFdHandle::new(pid);
        let child_parent = if params.thread() || params.inherit_parent() {
            parent.identity.parent
        } else {
            Some(parent.identity.thread_group)
        };

        let mut child_task = if params.shares_vm() {
            parent.task.fork_shared_vm().map_err(SysErr::from)?
        } else {
            parent.task.fork_cow().map_err(SysErr::from)?
        };
        child_task.process.context_mut().set_return_value(0);
        if let Some(stack_pointer) = params.child_stack_pointer {
            child_task
                .process
                .context_mut()
                .set_stack_pointer(stack_pointer);
        }
        if params.set_tls()
            && let Some(tls) = params.tls
        {
            child_task.process.context_mut().set_thread_pointer(tls);
        }
        if params.set_child_tid()
            && let Some(child_tid) = params.child_tid
        {
            let raw = pid.to_ne_bytes();
            let written = child_task
                .address_space
                .write(child_tid, &raw)
                .map_err(SysErr::from)?;
            if written != raw.len() {
                return Err(SysErr::Fault);
            }
        }

        let mut child_prctl = parent.prctl;
        child_prctl.parent_death_signal = 0;
        child_prctl.child_subreaper = false;

        let vfork_parent = params.is_vfork().then_some(parent.identity.pid);
        let child_signals = if params.share_sighand() {
            parent.signals.clone_for_thread()
        } else {
            parent.signals.fork_copy()
        };
        let child_files = if params.share_files() {
            parent.files.clone()
        } else {
            ProcessManager::clone_fd_table_for_fork(&parent.files, &child_signals)
        };
        let child_fs = if params.share_fs() {
            parent.fs.clone()
        } else {
            parent.fs.fork_copy()
        };
        let assigned_cpu = if params.shares_vm() {
            parent.assigned_cpu
        } else {
            crate::processor::select_cpu_for_child(parent.assigned_cpu)
        };
        if params.set_parent_tid()
            && let Some(parent_tid) = params.parent_tid
        {
            let raw = pid.to_ne_bytes();
            let written = parent
                .task
                .address_space
                .write(parent_tid, &raw)
                .map_err(SysErr::from)?;
            if written != raw.len() {
                return Err(SysErr::Fault);
            }
        }
        if let Some(pidfd_address) = params.pidfd_address {
            let pidfd_node = create_pidfd_node(pidfd.clone());
            let pidfd_number = parent.files.insert_node(
                pidfd_node,
                OpenFlags::from_bits(OpenFlags::READ),
                anonymous_filesystem_identity(),
                None,
                true,
            );
            let raw = (pidfd_number as i32).to_ne_bytes();
            let written = parent
                .task
                .address_space
                .write(pidfd_address, &raw)
                .map_err(SysErr::from)?;
            if written != raw.len() {
                let _ = parent.files.close(pidfd_number);
                return Err(SysErr::Fault);
            }
        }

        let child = KernelProcess {
            identity: crate::process::ProcessIdentity {
                pid,
                thread_group: if params.thread() {
                    parent.identity.thread_group
                } else {
                    pid
                },
                parent: child_parent,
                process_group: parent.identity.process_group,
                session: parent.identity.session,
            },
            pidfd,
            exit_signal: if params.thread() {
                0
            } else {
                params.exit_signal as u8
            },
            task: child_task,
            credentials: parent.credentials.clone(),
            prctl: child_prctl,
            assigned_cpu,
            kernel_context: None,
            kernel_cpu: None,
            pending_exec: None,
            pending_syscall: None,
            pending_syscall_name: "",
            pending_file_waits: Vec::new(),
            mmap_regions: parent.mmap_regions.clone(),
            vfork_parent,
            set_child_tid: None,
            robust_list_head: None,
            robust_list_len: 0,
            clear_child_tid: params
                .clear_child_tid()
                .then_some(params.child_tid)
                .flatten(),
            files: child_files,
            fs: child_fs,
            umask: parent.umask,
            signals: child_signals,
            wake_result: None,
            state: ProcessState::Runnable,
        };

        Ok(self.processes.lock().insert_cloned_process(child))
    }

    fn reap_child_event(
        &mut self,
        parent_pid: Pid,
        requested: i32,
        options: u64,
    ) -> Option<ChildEvent> {
        self.processes
            .lock()
            .reap_child_event(parent_pid, requested, options)
    }

    fn has_child(&mut self, parent_pid: Pid, requested: i32) -> bool {
        self.processes.lock().has_child(parent_pid, requested)
    }

    fn thread_group_of(&mut self, pid: Pid) -> Option<Pid> {
        self.processes.lock().thread_group_of(pid)
    }

    fn has_thread_group(&mut self, tgid: Pid) -> bool {
        self.processes.lock().has_thread_group(tgid)
    }

    fn has_process_group(&mut self, process_group: Pid) -> bool {
        self.processes.lock().has_process_group(process_group)
    }

    fn wake_vfork_parent(&mut self, parent_pid: Pid, child_pid: Pid) {
        self.processes
            .lock()
            .wake_vfork_parent(parent_pid, child_pid);
    }

    fn send_kernel_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool {
        self.processes.lock().send_signal(pid, signal)
    }

    fn send_process_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool {
        self.processes.lock().send_signal_thread_group(pid, signal)
    }

    fn send_process_group_signal(
        &mut self,
        process_group: Pid,
        signal: crate::signal::SignalInfo,
    ) -> usize {
        self.processes
            .lock()
            .send_signal_process_group(process_group, signal)
    }

    fn send_signal_all(
        &mut self,
        signal: crate::signal::SignalInfo,
        exclude_tgid: Option<Pid>,
    ) -> usize {
        self.processes
            .lock()
            .send_signal_all_thread_groups(signal, exclude_tgid)
    }

    fn arm_futex_wait(&mut self, pid: Pid, key: crate::process::FutexKey, bitset: u32) {
        self.processes.lock().arm_futex_wait(pid, key, bitset);
    }

    fn disarm_futex_wait(&mut self, pid: Pid) {
        self.processes.lock().disarm_futex_wait(pid);
    }

    fn wake_futex(&mut self, key: crate::process::FutexKey, bitset: u32, count: usize) -> usize {
        self.processes.lock().wake_futex(key, bitset, count)
    }

    fn requeue_futex(
        &mut self,
        from: crate::process::FutexKey,
        to: crate::process::FutexKey,
        wake_count: usize,
        requeue_count: usize,
        bitset: u32,
    ) -> usize {
        self.processes
            .lock()
            .requeue_futex(from, to, wake_count, requeue_count, bitset)
    }

    fn log_unimplemented(&mut self, number: u64, name: &str, pid: u32, args: SyscallArgs) {
        log::warn!(
            "syscall: pid {} unimplemented nr={} {} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
            pid,
            number,
            name,
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
            args.get(5)
        );
    }

    fn log_unimplemented_command(
        &mut self,
        name: &str,
        command_name: &str,
        command: u64,
        pid: u32,
        args: SyscallArgs,
    ) {
        log::warn!(
            "syscall: pid {} unimplemented {} {}={:#x} args=[{:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x}]",
            pid,
            name,
            command_name,
            command,
            args.get(0),
            args.get(1),
            args.get(2),
            args.get(3),
            args.get(4),
            args.get(5)
        );
    }
}

fn build_initial_files(rootfs: &RootfsManager) -> Result<FdTable, RuntimeInitError> {
    let console = rootfs.device_namespace().lookup(rootfs.vfs(), "console");
    let kmsg = rootfs
        .device_namespace()
        .lookup(rootfs.vfs(), "kmsg")
        .ok_or(RuntimeInitError::FileSystem(FsError::NotFound))?;

    let stdin = console.clone().unwrap_or_else(|| kmsg.clone());
    let stdout = console.clone().unwrap_or_else(|| kmsg.clone());
    let stderr = console.unwrap_or(kmsg);

    Ok(FdTable::new_with_stdio(
        stdin,
        stdout,
        stderr,
        rootfs.device_filesystem_identity(),
    ))
}
