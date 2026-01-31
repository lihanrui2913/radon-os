use alloc::{
    collections::btree_map::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use namespace::protocol::MountFlags;
use radon_kernel::{EEXIST, EINVAL, ENOENT, ESRCH, Error, Result};
use spin::RwLock;

#[derive(Debug, Clone)]
pub struct NamespaceEntry {
    pub path: String,
    pub name: String,
    pub flags: MountFlags,
}

pub struct Namespace {
    mounts: RwLock<BTreeMap<String, NamespaceEntry>>,
}

impl Namespace {
    pub fn new() -> Self {
        Self {
            mounts: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn bind(&self, path: impl AsRef<str>, name: String, flags: MountFlags) -> Result<()> {
        let path = Self::normalize_path(path.as_ref())?;

        let entry = NamespaceEntry {
            path: path.clone(),
            name,
            flags,
        };

        let mut mounts = self.mounts.write();

        if mounts.contains_key(&path) {
            return Err(Error::new(EEXIST));
        }

        mounts.insert(path, entry);
        Ok(())
    }

    pub fn unbind(&self, path: impl AsRef<str>) -> Result<NamespaceEntry> {
        let path = Self::normalize_path(path.as_ref())?;

        self.mounts
            .write()
            .remove(&path)
            .ok_or_else(|| Error::new(ESRCH))
    }

    pub fn resolve(&self, path: impl AsRef<str>) -> Result<(NamespaceEntry, String)> {
        let path = Self::normalize_path(path.as_ref())?;
        let mounts = self.mounts.read();

        let mut best_match: Option<(&str, &NamespaceEntry)> = None;

        for (prefix, entry) in mounts.iter().rev() {
            if Self::path_starts_with(&path, prefix) {
                match &best_match {
                    None => best_match = Some((prefix, entry)),
                    Some((best_prefix, _)) => {
                        if prefix.len() > best_prefix.len() {
                            best_match = Some((prefix, entry));
                        }
                    }
                }
            }
        }

        match best_match {
            Some((prefix, entry)) => {
                let remaining = Self::strip_prefix(&path, prefix);
                Ok((entry.clone(), remaining))
            }
            None => Err(Error::new(ENOENT)),
        }
    }

    fn normalize_path(path: &str) -> Result<String> {
        if !path.starts_with('/') {
            return Err(Error::new(EINVAL));
        }

        let normalized = if path.len() > 1 && path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            path.to_string()
        };

        let mut components = Vec::new();
        for component in normalized.split('/') {
            match component {
                "" | "." => continue,
                ".." => {
                    components.pop();
                }
                c => components.push(c),
            }
        }

        if components.is_empty() {
            Ok("/".to_string())
        } else {
            Ok(format!("/{}", components.join("/")))
        }
    }

    fn path_starts_with(path: &str, prefix: &str) -> bool {
        if prefix == "/" {
            return true;
        }
        if path == prefix {
            return true;
        }
        path.starts_with(&format!("{}/", prefix))
    }

    fn strip_prefix(path: &str, prefix: &str) -> String {
        if prefix == "/" {
            return path.to_string();
        }
        if path == prefix {
            return "/".to_string();
        }
        path.strip_prefix(prefix).unwrap_or(path).to_string()
    }
}
