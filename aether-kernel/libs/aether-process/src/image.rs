extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    UnexpectedEof,
    InvalidElf,
    UnsupportedElf,
    ReadFailure,
}

pub trait ProgramImageSource {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_exact_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), ImageError>;

    fn read_alloc(&self, offset: usize, len: usize) -> Result<Vec<u8>, ImageError> {
        let mut bytes = vec![0; len];
        self.read_exact_at(offset, &mut bytes)?;
        Ok(bytes)
    }
}

impl ProgramImageSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn read_exact_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), ImageError> {
        let end = offset
            .checked_add(buffer.len())
            .ok_or(ImageError::UnexpectedEof)?;
        let slice = self.get(offset..end).ok_or(ImageError::UnexpectedEof)?;
        buffer.copy_from_slice(slice);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfImageType {
    Executable,
    SharedObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfSegmentType {
    Load,
    Interp,
    Phdr,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfSegmentFlags(u32);

impl ElfSegmentFlags {
    pub const EXECUTE: u32 = 0x1;
    pub const WRITE: u32 = 0x2;
    pub const READ: u32 = 0x4;

    pub const fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn is_read(self) -> bool {
        (self.0 & Self::READ) != 0
    }

    pub const fn is_write(self) -> bool {
        (self.0 & Self::WRITE) != 0
    }

    pub const fn is_execute(self) -> bool {
        (self.0 & Self::EXECUTE) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfProgramHeader {
    pub kind: ElfSegmentType,
    pub flags: ElfSegmentFlags,
    pub offset: u64,
    pub virtual_addr: u64,
    pub file_size: u64,
    pub mem_size: u64,
}

pub struct ElfImage {
    image_type: ElfImageType,
    entry: u64,
    phoff: u64,
    phnum: u64,
    phent: u64,
    min_vaddr: u64,
    program_headers: Vec<ElfProgramHeader>,
}

impl ElfImage {
    pub fn parse<S: ProgramImageSource + ?Sized>(source: &S) -> Result<Self, ImageError> {
        let mut header = [0u8; 64];
        source.read_exact_at(0, &mut header)?;

        if &header[..4] != b"\x7fELF" {
            return Err(ImageError::InvalidElf);
        }
        if header[4] != 2 || header[5] != 1 || header[6] != 1 {
            return Err(ImageError::UnsupportedElf);
        }

        let image_type = match read_u16(&header, 16)? {
            2 => ElfImageType::Executable,
            3 => ElfImageType::SharedObject,
            _ => return Err(ImageError::UnsupportedElf),
        };

        let entry = read_u64(&header, 24)?;
        let phoff = read_u64(&header, 32)?;
        let phent = read_u16(&header, 54)? as u64;
        let phnum = read_u16(&header, 56)? as u64;
        if phnum == 0 || phent < 56 {
            return Err(ImageError::InvalidElf);
        }

        let ph_table_len = phent.checked_mul(phnum).ok_or(ImageError::InvalidElf)? as usize;
        let ph_bytes = source.read_alloc(phoff as usize, ph_table_len)?;

        let mut program_headers = Vec::with_capacity(phnum as usize);
        let mut min_vaddr = u64::MAX;
        let mut max_vaddr = 0u64;

        for index in 0..phnum as usize {
            let start = index
                .checked_mul(phent as usize)
                .ok_or(ImageError::InvalidElf)?;
            let entry_bytes = ph_bytes
                .get(start..start + phent as usize)
                .ok_or(ImageError::InvalidElf)?;

            let kind = match read_u32(entry_bytes, 0)? {
                1 => ElfSegmentType::Load,
                3 => ElfSegmentType::Interp,
                6 => ElfSegmentType::Phdr,
                other => ElfSegmentType::Other(other),
            };
            let flags = ElfSegmentFlags::new(read_u32(entry_bytes, 4)?);
            let offset = read_u64(entry_bytes, 8)?;
            let virtual_addr = read_u64(entry_bytes, 16)?;
            let file_size = read_u64(entry_bytes, 32)?;
            let mem_size = read_u64(entry_bytes, 40)?;

            if kind == ElfSegmentType::Load {
                let start = align_down(virtual_addr, 4096);
                let end = align_up(
                    virtual_addr
                        .checked_add(mem_size)
                        .ok_or(ImageError::InvalidElf)?,
                    4096,
                );
                min_vaddr = min_vaddr.min(start);
                max_vaddr = max_vaddr.max(end);
            }

            program_headers.push(ElfProgramHeader {
                kind,
                flags,
                offset,
                virtual_addr,
                file_size,
                mem_size,
            });
        }

        if min_vaddr == u64::MAX || max_vaddr <= min_vaddr {
            return Err(ImageError::InvalidElf);
        }

        Ok(Self {
            image_type,
            entry,
            phoff,
            phnum,
            phent,
            min_vaddr,
            program_headers,
        })
    }

    pub fn entry(&self) -> u64 {
        self.entry
    }

    pub fn phnum(&self) -> u64 {
        self.phnum
    }

    pub fn phent(&self) -> u64 {
        self.phent
    }

    pub fn program_headers(&self) -> &[ElfProgramHeader] {
        &self.program_headers
    }

    pub fn load_bias(&self, preferred_base: u64) -> u64 {
        match self.image_type {
            ElfImageType::SharedObject => preferred_base.saturating_sub(self.min_vaddr),
            ElfImageType::Executable => 0,
        }
    }

    pub fn interpreter_path<S: ProgramImageSource + ?Sized>(
        &self,
        source: &S,
    ) -> Result<Option<String>, ImageError> {
        let Some(header) = self
            .program_headers
            .iter()
            .find(|header| header.kind == ElfSegmentType::Interp)
        else {
            return Ok(None);
        };

        let path = source.read_alloc(header.offset as usize, header.file_size as usize)?;
        let nul = path
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(path.len());
        String::from_utf8(path[..nul].to_vec())
            .map(Some)
            .map_err(|_| ImageError::InvalidElf)
    }

    pub fn phdr_addr(&self, load_bias: u64) -> Result<u64, ImageError> {
        if let Some(header) = self
            .program_headers
            .iter()
            .find(|header| header.kind == ElfSegmentType::Phdr)
        {
            return Ok(load_bias + header.virtual_addr);
        }

        for header in &self.program_headers {
            if header.kind != ElfSegmentType::Load {
                continue;
            }

            let file_start = header.offset;
            let file_end = header
                .offset
                .checked_add(header.file_size)
                .ok_or(ImageError::InvalidElf)?;
            if self.phoff >= file_start && self.phoff < file_end {
                return Ok(load_bias + header.virtual_addr + (self.phoff - file_start));
            }
        }

        Err(ImageError::InvalidElf)
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ImageError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or(ImageError::UnexpectedEof)?;
    let mut value = [0u8; 2];
    value.copy_from_slice(raw);
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(ImageError::UnexpectedEof)?;
    let mut value = [0u8; 4];
    value.copy_from_slice(raw);
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ImageError> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(ImageError::UnexpectedEof)?;
    let mut value = [0u8; 8];
    value.copy_from_slice(raw);
    Ok(u64::from_le_bytes(value))
}

fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}
