extern crate alloc;

use alloc::collections::BTreeMap;
use core::cmp::min;
use core::ptr;

use aether_frame::boot::phys_to_virt;
use aether_frame::libs::spin::SpinLock;
use aether_frame::mm::{
    FrameAllocator, PAGE_SIZE, PhysFrame, frame_allocator, release_frames, zero_frame,
};

use crate::{FileAdvice, FileOperations, FsError, FsResult, NodeMetadata};

const PAGE_CACHE_MAX_PAGES: usize = 8192;
const PAGE_PREFETCH_LIMIT: usize = 16;

static PAGE_CACHE: SpinLock<PageCacheState> = SpinLock::new(PageCacheState::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CachedFileId {
    device_id: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PageCacheKey {
    file: CachedFileId,
    page_index: u64,
}

#[derive(Debug, Clone, Copy)]
struct CachedPage {
    frame: PhysFrame,
    len: usize,
    last_access: u64,
}

struct PageCacheState {
    entries: BTreeMap<PageCacheKey, CachedPage>,
    access_epoch: u64,
}

impl PageCacheState {
    const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            access_epoch: 1,
        }
    }

    fn next_access(&mut self) -> u64 {
        let access = self.access_epoch;
        self.access_epoch = self.access_epoch.saturating_add(1);
        access
    }

    fn cached_page(&mut self, key: &PageCacheKey) -> Option<CachedPage> {
        let access = self.next_access();
        let page = self.entries.get_mut(key)?;
        page.last_access = access;
        Some(*page)
    }

    fn copy_cached(
        &mut self,
        key: &PageCacheKey,
        page_offset: usize,
        buffer: &mut [u8],
    ) -> Option<(usize, bool)> {
        let page = self.cached_page(key)?;
        if page.len <= page_offset {
            return Some((0, true));
        }

        let chunk = min(buffer.len(), page.len - page_offset);
        copy_from_frame(page.frame, page_offset, &mut buffer[..chunk]);
        Some((chunk, page.len < PAGE_SIZE as usize))
    }

    fn contains(&self, key: &PageCacheKey) -> bool {
        self.entries.contains_key(key)
    }

    fn insert_page(&mut self, key: PageCacheKey, frame: PhysFrame, len: usize) -> CachedPage {
        let access = self.next_access();
        if let Some(existing) = self.entries.get_mut(&key) {
            existing.last_access = access;
            return *existing;
        }

        while self.entries.len() >= PAGE_CACHE_MAX_PAGES {
            if !self.evict_one() {
                break;
            }
        }

        let page = CachedPage {
            frame,
            len,
            last_access: access,
        };
        self.entries.insert(key, page);
        page
    }

    fn evict_one(&mut self) -> bool {
        let Some((&oldest_key, oldest_page)) = self
            .entries
            .iter()
            .min_by_key(|(_, page)| page.last_access)
            .map(|(key, page)| (key, *page))
        else {
            return false;
        };

        let _ = self.entries.remove(&oldest_key);
        release_frames(oldest_page.frame, 1);
        true
    }
}

pub(crate) fn read(
    metadata: NodeMetadata,
    operations: &dyn FileOperations,
    offset: usize,
    buffer: &mut [u8],
) -> FsResult<usize> {
    let Some(file_id) = cacheable_file_id(metadata, operations) else {
        return operations.read(offset, buffer);
    };
    if buffer.is_empty() {
        return Ok(0);
    }

    let mut copied = 0usize;
    while copied < buffer.len() {
        let absolute = offset.checked_add(copied).ok_or(FsError::InvalidInput)?;
        let page_index = (absolute / PAGE_SIZE as usize) as u64;
        let page_offset = absolute % PAGE_SIZE as usize;
        let key = PageCacheKey {
            file: file_id,
            page_index,
        };

        if let Some((chunk, short_page)) =
            PAGE_CACHE
                .lock()
                .copy_cached(&key, page_offset, &mut buffer[copied..])
        {
            if chunk == 0 {
                break;
            }
            copied += chunk;
            if short_page {
                break;
            }
            continue;
        }

        if load_page(operations, key)?.is_none() {
            break;
        }
    }

    Ok(copied)
}

pub(crate) fn invalidate_write(
    metadata: NodeMetadata,
    operations: &dyn FileOperations,
    offset: usize,
    len: usize,
) {
    let Some(file_id) = cacheable_file_id(metadata, operations) else {
        return;
    };
    invalidate_file_range(file_id, offset, len);
}

pub(crate) fn invalidate_all(metadata: NodeMetadata, operations: &dyn FileOperations) {
    let Some(file_id) = cacheable_file_id(metadata, operations) else {
        return;
    };

    let mut state = PAGE_CACHE.lock();
    state.entries.retain(|key, page| {
        if key.file == file_id {
            release_frames(page.frame, 1);
            false
        } else {
            true
        }
    });
}

pub(crate) fn handle_advice(
    metadata: NodeMetadata,
    operations: &dyn FileOperations,
    offset: u64,
    len: u64,
    advice: FileAdvice,
) {
    let Some(file_id) = cacheable_file_id(metadata, operations) else {
        return;
    };

    match advice {
        FileAdvice::DontNeed => invalidate_file_range(
            file_id,
            offset.min(usize::MAX as u64) as usize,
            len.min(usize::MAX as u64) as usize,
        ),
        FileAdvice::WillNeed => prefetch_range(
            operations,
            file_id,
            offset.min(usize::MAX as u64) as usize,
            len.min(usize::MAX as u64) as usize,
        ),
        _ => {}
    }
}

pub fn reclaim_page_cache() -> usize {
    let Some(mut state) = PAGE_CACHE.try_lock() else {
        return 0;
    };

    let entries = core::mem::take(&mut state.entries);
    state.access_epoch = 1;
    drop(state);

    let freed = entries.len();
    for (_, page) in entries {
        release_frames(page.frame, 1);
    }
    freed
}

fn cacheable_file_id(
    metadata: NodeMetadata,
    operations: &dyn FileOperations,
) -> Option<CachedFileId> {
    if !operations.page_cache_enabled() {
        return None;
    }

    Some(CachedFileId {
        device_id: metadata.device_id,
        inode: metadata.inode,
    })
}

fn prefetch_range(
    operations: &dyn FileOperations,
    file_id: CachedFileId,
    offset: usize,
    len: usize,
) {
    if len == 0 {
        return;
    }

    let first_page = offset / PAGE_SIZE as usize;
    let page_count = len.div_ceil(PAGE_SIZE as usize).min(PAGE_PREFETCH_LIMIT);
    for page in 0..page_count {
        let key = PageCacheKey {
            file: file_id,
            page_index: (first_page + page) as u64,
        };
        if PAGE_CACHE.lock().contains(&key) {
            continue;
        }
        let _ = load_page(operations, key);
    }
}

fn invalidate_file_range(file_id: CachedFileId, offset: usize, len: usize) {
    if len == 0 {
        return;
    }

    let start_page = offset / PAGE_SIZE as usize;
    let end_page = offset.saturating_add(len.saturating_sub(1)) / PAGE_SIZE as usize;

    let mut state = PAGE_CACHE.lock();
    state.entries.retain(|key, page| {
        if key.file == file_id
            && key.page_index >= start_page as u64
            && key.page_index <= end_page as u64
        {
            release_frames(page.frame, 1);
            false
        } else {
            true
        }
    });
}

fn load_page(operations: &dyn FileOperations, key: PageCacheKey) -> FsResult<Option<CachedPage>> {
    let frame = match frame_allocator().lock().alloc(1) {
        Ok(frame) => frame,
        Err(_) => return Ok(None),
    };
    zero_frame(frame);

    let buffer = unsafe {
        core::slice::from_raw_parts_mut(
            phys_to_virt(frame.start_address().as_u64()) as *mut u8,
            PAGE_SIZE as usize,
        )
    };

    let mut read = 0usize;
    while read < buffer.len() {
        let offset = key.page_index as usize * PAGE_SIZE as usize + read;
        let chunk = match operations.read(offset, &mut buffer[read..]) {
            Ok(chunk) => chunk,
            Err(error) => {
                release_frames(frame, 1);
                return Err(error);
            }
        };
        if chunk == 0 {
            break;
        }
        read += chunk;
    }

    if read == 0 {
        release_frames(frame, 1);
        return Ok(None);
    }

    let inserted = PAGE_CACHE.lock().insert_page(key, frame, read);
    if inserted.frame != frame {
        release_frames(frame, 1);
    }
    Ok(Some(inserted))
}

fn copy_from_frame(frame: PhysFrame, offset: usize, buffer: &mut [u8]) {
    unsafe {
        ptr::copy_nonoverlapping(
            (phys_to_virt(frame.start_address().as_u64()) as *const u8).add(offset),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
    }
}
