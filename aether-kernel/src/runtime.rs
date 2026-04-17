extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use aether_device::{DeviceRegistry, KernelDevice};
use aether_drivers::DriverInventory;
use aether_frame::boot;
use aether_frame::libs::spin::SpinLock;
use aether_frame::logger::RegisterWriterError;
use aether_framebuffer::{FramebufferDevice, FramebufferError, FramebufferSurface, RgbColor};
use aether_kmsg::KmsgBuffer;
use aether_process::{BuildError, ElfProgramBuilder, UserProgramBuilder, elf_interpreter_path};
use aether_terminal::FramebufferConsole;
use aether_vfs::{FileNode, FsError, NodeRef};

use crate::arch::ArchContext;
use crate::errno::{SysErr, SysResult};
use crate::fs::{FdTable, NodeImageSource};
use crate::log_sinks;
use crate::process::{
    ChildEvent, CloneParams, DispatchWork, KernelProcess, Pid, ProcFsProcessSnapshot, ProcessBlock,
    ProcessManager, ProcessServices, ProcessState, ScheduleEvent,
};
use crate::rootfs::{ExecFormat, ProcessFsContext, RootfsError, RootfsManager};
use crate::syscall::BlockType;
use crate::syscall::SyscallArgs;

#[derive(Debug)]
pub enum RuntimeInitError {
    FileSystem(FsError),
    Framebuffer(FramebufferError),
    LogWriter(RegisterWriterError),
    Process(BuildError),
    Rootfs(RootfsError),
}

static CURRENT_PIDS: SpinLock<[Option<Pid>; boot::MAX_CPUS]> =
    SpinLock::new([None; boot::MAX_CPUS]);

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

static SHARED_RUNTIME: SpinLock<Option<Arc<KernelRuntime>>> = SpinLock::new(None);
static TIMER_TICK_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT_TIMER_DEADLINE_NANOS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);

pub(crate) fn publish_next_timer_deadline(deadline_nanos: Option<u64>) {
    NEXT_TIMER_DEADLINE_NANOS.store(deadline_nanos.unwrap_or(u64::MAX), Ordering::Release);
    if deadline_nanos.is_none() {
        TIMER_TICK_PENDING.store(false, Ordering::Release);
    }
}

fn runtime_tick_handler() {
    let deadline_nanos = NEXT_TIMER_DEADLINE_NANOS.load(Ordering::Acquire);
    let process_due = deadline_nanos != u64::MAX
        && aether_frame::interrupt::timer::nanos_since_boot() >= deadline_nanos;
    if process_due || crate::fs::timerfd_deadline_due() {
        TIMER_TICK_PENDING.store(true, Ordering::Release);
    }
}

fn block_type_to_process_block(block: BlockType) -> ProcessBlock {
    match block {
        BlockType::Timer {
            target_nanos,
            request_nanos,
            rmtp,
            flags,
        } => ProcessBlock::Timer {
            target_nanos,
            request_nanos,
            rmtp,
            flags,
        },
        BlockType::File { fd, events } => ProcessBlock::File { fd, events },
        BlockType::Poll { deadline_nanos } => ProcessBlock::Poll { deadline_nanos },
        BlockType::Futex { uaddr, bitset } => ProcessBlock::Futex { uaddr, bitset },
        BlockType::SignalSuspend => ProcessBlock::SignalSuspend,
        BlockType::Vfork { child } => ProcessBlock::Vfork { child },
        BlockType::WaitChild {
            pid,
            status_ptr,
            options,
        } => ProcessBlock::WaitChild {
            pid,
            status_ptr,
            options,
        },
    }
}

fn process_kernel_syscall_entry(process: &mut KernelProcess) {
    let runtime = KernelRuntime::shared().expect("kernel syscall continuation requires runtime");

    loop {
        let dispatch = ProcessManager::dispatch_pending_syscall_direct(
            process,
            RuntimeServices {
                rootfs: &runtime.rootfs,
                processes: &runtime.processes,
            },
        );

        match dispatch.disposition {
            crate::syscall::SyscallDisposition::Return(value) => {
                if matches!(process.state, ProcessState::Running) {
                    process.wake_result = None;
                    if process.pending_exec.is_none() {
                        process
                            .task
                            .process
                            .context_mut()
                            .set_return_value(match value {
                                Ok(value) => value,
                                Err(error) => error.errno() as u64,
                            });
                    }
                    process.state = ProcessState::Runnable;
                }
                break;
            }
            crate::syscall::SyscallDisposition::Exit(status) => {
                process.state = ProcessState::Exited(status);
                break;
            }
            crate::syscall::SyscallDisposition::Block(block) => {
                process.wake_result = None;
                process.state = ProcessState::Blocked(block_type_to_process_block(block));
                aether_frame::process::switch_to_scheduler(
                    process
                        .kernel_context
                        .as_mut()
                        .expect("blocked syscall missing kernel context"),
                );
            }
        }
    }

    aether_frame::process::switch_to_scheduler(
        process
            .kernel_context
            .as_mut()
            .expect("completed syscall missing kernel context"),
    );
}

impl KernelRuntime {
    pub fn current_pid() -> Option<Pid> {
        CURRENT_PIDS.lock_irqsave()[aether_frame::arch::cpu::current_cpu_index()]
    }

    pub fn procfs_snapshot(pid: Pid) -> Option<ProcFsProcessSnapshot> {
        Self::shared().and_then(|runtime| runtime.processes.lock_irqsave().procfs_snapshot(pid))
    }

    pub fn bootstrap() -> Result<Self, RuntimeInitError> {
        log_sinks::init()?;
        crate::syscall::init();
        crate::net::init();

        let rootfs = Arc::new(RootfsManager::new()?);
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
            console = Some(terminal);
        } else {
            log::warn!("bootloader did not provide a framebuffer");
        }

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
        processes.set_initial_fs(initial_fs.clone());
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
        *SHARED_RUNTIME.lock_irqsave() = Some(runtime.clone());
        aether_frame::interrupt::timer::register_tick_handler(runtime_tick_handler);
        runtime
    }

    pub fn shared() -> Option<Arc<Self>> {
        SHARED_RUNTIME.lock_irqsave().clone()
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
        let mut scheduler_context = aether_frame::process::KernelContext::default();
        aether_frame::process::install_scheduler_context(&mut scheduler_context);
        loop {
            aether_frame::executor::run_ready();
            if aether_frame::preempt::take_need_resched() {
                continue;
            }
            let work = {
                let mut processes = self.processes.lock_irqsave();
                processes.wake_ready_file_blocks();
                if TIMER_TICK_PENDING.swap(false, Ordering::AcqRel) {
                    crate::fs::wake_expired_timerfds();
                    let current_nanos = aether_frame::interrupt::timer::nanos_since_boot();
                    if processes.timer_deadline_due(current_nanos) {
                        processes.wake_expired_timers(current_nanos);
                    }
                }
                processes.take_next_process()
            };
            match work {
                DispatchWork::Idle => {
                    if !aether_frame::preempt::take_need_resched() {
                        aether_frame::arch::cpu::wait_for_interrupt();
                    }
                }
                DispatchWork::Event(event) => self.handle_event(event),
                DispatchWork::Process(mut process) => {
                    CURRENT_PIDS.lock_irqsave()[aether_frame::arch::cpu::current_cpu_index()] =
                        Some(process.identity.pid);
                    let result = process.task.process.run();
                    let event = match result.reason {
                        aether_frame::process::RunReason::Syscall => {
                            let syscall_number = result.context.syscall_number();
                            let syscall_args = SyscallArgs::from_context(&result.context);
                            {
                                let mut processes = self.processes.lock_irqsave();
                                processes.mark_process_ran(process.identity.pid);
                            }
                            process.pending_syscall = Some(crate::process::PendingSyscall {
                                number: syscall_number,
                                args: syscall_args,
                            });
                            process.pending_syscall_name = "";
                            process.kernel_cpu = Some(aether_frame::arch::cpu::current_cpu_index());
                            if process.kernel_context.is_none() {
                                process.kernel_context =
                                    Some(aether_frame::process::initialize_typed_kernel_context(
                                        process.task.process.kernel_stack_top(),
                                        process.as_mut(),
                                        process_kernel_syscall_entry,
                                    ));
                            }
                            aether_frame::process::resume_kernel_context(
                                &mut scheduler_context,
                                process
                                    .kernel_context
                                    .as_ref()
                                    .expect("syscall continuation missing kernel context"),
                            );
                            let mut processes = self.processes.lock_irqsave();
                            processes.finish_kernel_syscall_context(process)
                        }
                        _ => {
                            let services = RuntimeServices {
                                rootfs: &self.rootfs,
                                processes: &self.processes,
                            };
                            let mut processes = self.processes.lock_irqsave();
                            processes.finish_process(process, result, services)
                        }
                    };
                    CURRENT_PIDS.lock_irqsave()[aether_frame::arch::cpu::current_cpu_index()] =
                        None;
                    self.handle_event(event);
                }
                DispatchWork::KernelSyscall(process) => {
                    CURRENT_PIDS.lock_irqsave()[aether_frame::arch::cpu::current_cpu_index()] =
                        Some(process.identity.pid);
                    aether_frame::process::resume_kernel_context(
                        &mut scheduler_context,
                        process
                            .kernel_context
                            .as_ref()
                            .expect("resumed syscall missing kernel context"),
                    );
                    let event = {
                        let mut processes = self.processes.lock_irqsave();
                        processes.finish_kernel_syscall_context(process)
                    };
                    CURRENT_PIDS.lock_irqsave()[aether_frame::arch::cpu::current_cpu_index()] =
                        None;
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
        self.processes.lock_irqsave().clone_process(parent, params)
    }

    fn reap_child_event(
        &mut self,
        parent_pid: Pid,
        requested: i32,
        options: u64,
    ) -> Option<ChildEvent> {
        self.processes
            .lock_irqsave()
            .reap_child_event(parent_pid, requested, options)
    }

    fn has_child(&mut self, parent_pid: Pid, requested: i32) -> bool {
        self.processes
            .lock_irqsave()
            .has_child(parent_pid, requested)
    }

    fn wake_vfork_parent(&mut self, parent_pid: Pid, child_pid: Pid) {
        self.processes
            .lock_irqsave()
            .wake_vfork_parent(parent_pid, child_pid);
    }

    fn send_kernel_signal(&mut self, pid: Pid, signal: crate::signal::SignalInfo) -> bool {
        self.processes.lock_irqsave().send_signal(pid, signal)
    }

    fn wake_futex(&mut self, uaddr: u64, bitset: u32, count: usize) -> usize {
        self.processes
            .lock_irqsave()
            .wake_futex(uaddr, bitset, count)
    }

    fn requeue_futex(
        &mut self,
        from: u64,
        to: u64,
        wake_count: usize,
        requeue_count: usize,
        bitset: u32,
    ) -> usize {
        self.processes
            .lock_irqsave()
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
