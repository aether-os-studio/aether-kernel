use super::*;

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(super) fn syscall_read_user_c_string(
        &self,
        address: u64,
        limit: usize,
    ) -> SysResult<String> {
        self.process
            .task
            .address_space
            .read_c_string(address, limit)
            .map_err(|_| SysErr::Fault)
    }

    pub(super) fn syscall_read_user_buffer(&self, address: u64, len: usize) -> SysResult<Vec<u8>> {
        let mut buffer = vec![0; len];
        let read = self
            .process
            .task
            .address_space
            .read(address, &mut buffer)
            .map_err(|_| SysErr::Fault)?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub(crate) fn syscall_read_user_exact_buffer(
        &self,
        address: u64,
        len: usize,
    ) -> SysResult<Vec<u8>> {
        self.process
            .task
            .address_space
            .read_user_exact(address, len)
            .map_err(|_| SysErr::Fault)
    }

    pub(super) fn syscall_read_user_pointer_array(
        &self,
        address: u64,
        limit: usize,
    ) -> SysResult<Vec<u64>> {
        let mut pointers = Vec::with_capacity(limit);
        for index in 0..limit {
            let element_addr = address
                .checked_add((index * core::mem::size_of::<u64>()) as u64)
                .ok_or(SysErr::Fault)?;
            let bytes = self
                .process
                .task
                .address_space
                .read_user_exact(element_addr, core::mem::size_of::<u64>())
                .map_err(|_| SysErr::Fault)?;
            let value = u64::from_ne_bytes(bytes.as_slice().try_into().map_err(|_| SysErr::Fault)?);
            if value == 0 {
                break;
            }
            pointers.push(value);
        }
        Ok(pointers)
    }

    pub(super) fn syscall_write_user_buffer(
        &mut self,
        address: u64,
        bytes: &[u8],
    ) -> SysResult<()> {
        let written = self
            .process
            .task
            .address_space
            .write(address, bytes)
            .map_err(SysErr::from)?;
        if written != bytes.len() {
            return Err(SysErr::Fault);
        }
        Ok(())
    }

    pub(super) fn syscall_write_user_timespec(
        &mut self,
        address: u64,
        secs: i64,
        nanos: i64,
    ) -> SysResult<()> {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&secs.to_ne_bytes());
        bytes[8..].copy_from_slice(&nanos.to_ne_bytes());
        self.syscall_write_user_buffer(address, &bytes)
    }
}
