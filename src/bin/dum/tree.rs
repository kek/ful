//! Arena tree of scanned filesystem nodes with live size rollups.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub type NodeId = usize;

pub struct Node {
    pub name: OsString,
    /// Allocated bytes; for dirs this includes the whole subtree.
    pub size: u64,
    pub is_dir: bool,
    pub denied: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

pub struct Tree {
    nodes: Vec<Node>,
    by_path: HashMap<PathBuf, NodeId>,
    pub root: NodeId,
}

impl Tree {
    /// Root node's `name` is the full root path, so `path_of` works uniformly.
    pub fn new(root_path: &Path) -> Self {
        let root = Node {
            name: root_path.as_os_str().to_os_string(),
            size: 0,
            is_dir: true,
            denied: false,
            parent: None,
            children: Vec::new(),
        };
        let mut by_path = HashMap::new();
        by_path.insert(root_path.to_path_buf(), 0);
        Tree { nodes: vec![root], by_path, root: 0 }
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn lookup(&self, path: &Path) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    /// Insert a node and roll its size up through all ancestors.
    pub fn insert(
        &mut self,
        parent: NodeId,
        path: PathBuf,
        name: OsString,
        size: u64,
        is_dir: bool,
    ) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name,
            size,
            is_dir,
            denied: false,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        self.by_path.insert(path, id);
        self.bubble(Some(parent), size as i64);
        id
    }

    /// Set a node's size; returns the signed delta, already bubbled to ancestors.
    pub fn set_size(&mut self, id: NodeId, new_size: u64) -> i64 {
        let delta = new_size as i64 - self.nodes[id].size as i64;
        self.nodes[id].size = new_size;
        let parent = self.nodes[id].parent;
        self.bubble(parent, delta);
        delta
    }

    /// Detach a subtree; returns the (negative) signed delta applied to ancestors.
    /// Arena slots are not reclaimed (acceptable for v1); the path index is purged.
    pub fn remove(&mut self, id: NodeId, path: &Path) -> i64 {
        let size = self.nodes[id].size;
        if let Some(p) = self.nodes[id].parent {
            self.nodes[p].children.retain(|&c| c != id);
            self.bubble(Some(p), -(size as i64));
        }
        self.by_path.retain(|p, _| !p.starts_with(path));
        -(size as i64)
    }

    /// Node ids from `id` up to and including the root.
    pub fn ancestors_inclusive(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            out.push(i);
            cur = self.nodes[i].parent;
        }
        out
    }

    pub fn path_of(&self, id: NodeId) -> PathBuf {
        let ids = self.ancestors_inclusive(id);
        let mut path = PathBuf::new();
        for &i in ids.iter().rev() {
            path.push(&self.nodes[i].name);
        }
        path
    }

    fn bubble(&mut self, mut cur: Option<NodeId>, delta: i64) {
        while let Some(id) = cur {
            let s = self.nodes[id].size as i64 + delta;
            self.nodes[id].size = s.max(0) as u64;
            cur = self.nodes[id].parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tree() -> Tree {
        Tree::new(Path::new("/root"))
    }

    #[test]
    fn new_tree_has_indexed_root() {
        let t = tree();
        assert_eq!(t.lookup(Path::new("/root")), Some(t.root));
        assert_eq!(t.get(t.root).size, 0);
        assert!(t.get(t.root).is_dir);
    }

    #[test]
    fn insert_rolls_size_up_to_ancestors() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 100, false);
        assert_eq!(t.get(dir).size, 100);
        assert_eq!(t.get(t.root).size, 100);
    }

    #[test]
    fn set_size_returns_signed_delta_and_bubbles() {
        let mut t = tree();
        let f = t.insert(t.root, PathBuf::from("/root/f"), OsString::from("f"), 100, false);
        let d = t.set_size(f, 250);
        assert_eq!(d, 150);
        assert_eq!(t.get(t.root).size, 250);
        let d = t.set_size(f, 50);
        assert_eq!(d, -200);
        assert_eq!(t.get(t.root).size, 50);
    }

    #[test]
    fn remove_subtracts_subtree_and_purges_index() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 100, false);
        let delta = t.remove(dir, Path::new("/root/a"));
        assert_eq!(delta, -100);
        assert_eq!(t.get(t.root).size, 0);
        assert_eq!(t.lookup(Path::new("/root/a")), None);
        assert_eq!(t.lookup(Path::new("/root/a/f")), None);
        assert!(t.get(t.root).children.is_empty());
    }

    #[test]
    fn path_of_reconstructs_full_path() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        let f = t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 1, false);
        assert_eq!(t.path_of(f), PathBuf::from("/root/a/f"));
    }

    #[test]
    fn ancestors_inclusive_walks_to_root() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        let f = t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 1, false);
        assert_eq!(t.ancestors_inclusive(f), vec![f, dir, t.root]);
    }
}
