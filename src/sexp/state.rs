use std::collections::BTreeMap;

use crate::sexp::core::{DocCore, NodeIndex};

use wabi_tree::OSBTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollapseState {
    Collapsed,
    Expanded,
}

pub struct DocState {
    pub core: DocCore,
    pub starts_of_logical_lines: OSBTreeMap<NodeIndex, (NodeIndex, usize)>,
    pub collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
}

impl DocState {
    pub fn new() -> Self {
        DocState {
            core: DocCore::new(),
            starts_of_logical_lines: OSBTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
        }
    }
}
