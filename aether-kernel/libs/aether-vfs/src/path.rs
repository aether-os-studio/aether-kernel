extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::{DentryRef, NodeRef};

#[derive(Clone)]
pub struct VfsPath {
    path: String,
    node: NodeRef,
    dentry: Option<DentryRef>,
}

impl VfsPath {
    pub fn new(path: String, node: NodeRef) -> Self {
        Self {
            path: normalize_absolute_path(path.as_str()),
            node,
            dentry: None,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn node(&self) -> NodeRef {
        self.node.clone()
    }

    pub fn dentry(&self) -> Option<DentryRef> {
        self.dentry.clone()
    }

    pub fn from_dentry(path: String, dentry: DentryRef) -> Self {
        Self {
            path: normalize_absolute_path(path.as_str()),
            node: dentry.inode(),
            dentry: Some(dentry),
        }
    }

    pub fn rebind_path(&mut self, path: String) {
        self.path = normalize_absolute_path(path.as_str());
    }
}

pub fn split_components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .map(String::from)
        .collect()
}

pub fn normalize_absolute_path(path: &str) -> String {
    resolve_components(Vec::new(), 0, path)
}

pub fn resolve_view_path(root: &str, cwd: &str, path: &str) -> String {
    let root_components = split_components(root);
    if path.starts_with('/') {
        resolve_components(root_components.clone(), root_components.len(), path)
    } else {
        resolve_components(split_components(cwd), root_components.len(), path)
    }
}

pub fn resolve_namespace_path(base: &str, path: &str) -> String {
    if path.starts_with('/') {
        normalize_absolute_path(path)
    } else {
        resolve_components(split_components(base), 0, path)
    }
}

pub fn resolve_symlink_path(parent: &str, target: &str) -> String {
    resolve_namespace_path(parent, target)
}

pub fn parent_path(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
}

pub fn leaf_name(path: &str) -> &str {
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
}

pub fn is_within(root: &str, path: &str) -> bool {
    if root == "/" {
        return path.starts_with('/');
    }
    path == root
        || path
            .strip_prefix(root)
            .map(|suffix| suffix.starts_with('/'))
            .unwrap_or(false)
}

pub fn display_path_from_root(root: &str, path: &str) -> String {
    if root == "/" {
        return normalize_absolute_path(path);
    }
    if path == root {
        return String::from("/");
    }
    path.strip_prefix(root)
        .filter(|suffix| !suffix.is_empty())
        .map(String::from)
        .unwrap_or_else(|| String::from("/"))
}

pub fn remap_mount_path(path: &str, source: &str, target: &str) -> String {
    let path = normalize_absolute_path(path);
    let source = normalize_absolute_path(source);
    let target = normalize_absolute_path(target);

    if source == "/" {
        return if target == "/" {
            path
        } else if path == "/" {
            target
        } else {
            alloc::format!("{target}{path}")
        };
    }

    if path == source {
        return target;
    }

    if let Some(suffix) = path.strip_prefix(source.as_str())
        && suffix.starts_with('/')
    {
        return if target == "/" {
            String::from(suffix)
        } else {
            alloc::format!("{target}{suffix}")
        };
    }

    path
}

fn resolve_components(mut components: Vec<String>, anchor: usize, path: &str) -> String {
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.len() > anchor {
                    let _ = components.pop();
                }
            }
            other => components.push(String::from(other)),
        }
    }

    if components.is_empty() {
        String::from("/")
    } else {
        alloc::format!("/{}", components.join("/"))
    }
}
