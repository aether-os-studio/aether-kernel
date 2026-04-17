extern crate alloc;

use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub uid: u32,
    pub euid: u32,
    pub suid: u32,
    pub fsuid: u32,
    pub gid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub fsgid: u32,
    pub supplementary_groups: Vec<u32>,
}

impl Credentials {
    pub fn root() -> Self {
        Self {
            uid: 0,
            euid: 0,
            suid: 0,
            fsuid: 0,
            gid: 0,
            egid: 0,
            sgid: 0,
            fsgid: 0,
            supplementary_groups: Vec::new(),
        }
    }

    pub fn is_superuser(&self) -> bool {
        self.euid == 0
    }
}

impl Default for Credentials {
    fn default() -> Self {
        Self::root()
    }
}
