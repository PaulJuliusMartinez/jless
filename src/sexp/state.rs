use std::collections::BTreeMap;

use crate::sexp::core::{DocCore, NodeIndex};

use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};
use ocaml_sexplib::Ref;
use wabi_tree::OSBTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollapseState {
    Collapsed,
    Expanded,
}

pub struct DocState {
    tokenizer: BasicTapeTokenizer,
    pub core: DocCore,
    pub starts_of_logical_lines: OSBTreeMap<NodeIndex, (NodeIndex, usize)>,
    pub collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
}

impl DocState {
    pub fn new() -> Self {
        DocState {
            tokenizer: BasicTapeTokenizer::new(),
            core: DocCore::new(),
            starts_of_logical_lines: OSBTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        self.tokenizer.feed_more_data(data);
        self.process_additional_tokens(Some(data), false);
    }

    pub fn eof(&mut self) {
        self.tokenizer.eof();
        self.process_additional_tokens(None, true);
    }

    fn process_additional_tokens(&mut self, current_data: Option<&[u8]>, seen_eof: bool) {
        while let Some(witness) = self.tokenizer.has_enough_data_to_produce_tokens() {
            let current_data = current_data.map(Ref::Transient);
            match self.tokenizer.next_raw_token(witness, current_data) {
                Ok(Some(raw_token)) => self.core.append_raw_token(raw_token),
                Ok(None) => {
                    self.core.append_eof();
                    break;
                }
                Err(err) => {
                    self.core.append_tokenizer_error(err);
                    if seen_eof {
                        self.core.append_eof();
                    }
                    break;
                }
            }
        }
    }
}
