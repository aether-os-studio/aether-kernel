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
        const POINTERS_PER_CHUNK: usize = 16;
        const POINTER_SIZE: usize = core::mem::size_of::<u64>();
        let mut pointers = Vec::with_capacity(limit);
        let mut chunk = [0u8; POINTERS_PER_CHUNK * POINTER_SIZE];

        let mut index = 0usize;
        while index < limit {
            let remaining = limit - index;
            let chunk_len = remaining.min(POINTERS_PER_CHUNK);
            let byte_len = chunk_len * POINTER_SIZE;
            let element_addr = address
                .checked_add((index * POINTER_SIZE) as u64)
                .ok_or(SysErr::Fault)?;
            let bytes = self
                .process
                .task
                .address_space
                .read_user_exact(element_addr, byte_len)
                .map_err(|_| SysErr::Fault)?;
            chunk[..byte_len].copy_from_slice(bytes.as_slice());

            for entry in chunk[..byte_len].chunks_exact(POINTER_SIZE) {
                let value = u64::from_ne_bytes(entry.try_into().map_err(|_| SysErr::Fault)?);
                if value == 0 {
                    return Ok(pointers);
                }
                pointers.push(value);
            }
            index += chunk_len;
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
