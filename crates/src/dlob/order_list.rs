use std::collections::BinaryHeap;

use dashmap::DashMap;

use crate::dlob::dlob_node::{get_order_signature, DLOBNode, DirectionalNode, Node, SortDirection};

#[derive(Clone, Debug)]
pub struct Orderlist {
    pub bids: BinaryHeap<DirectionalNode>,
    pub asks: BinaryHeap<DirectionalNode>,
    pub order_sigs: DashMap<String, Node>,
    bid_sort_direction: SortDirection,
    ask_sort_direction: SortDirection,
}

impl Orderlist {
    pub fn new(bid_sort_direction: SortDirection, ask_sort_direction: SortDirection) -> Self {
        Orderlist {
            bids: BinaryHeap::new(),
            asks: BinaryHeap::new(),
            order_sigs: DashMap::new(),
            bid_sort_direction,
            ask_sort_direction,
        }
    }

    /// for debugging
    pub fn print(&self) {
        println!("Bids: {:?}", self.bids);
        println!("Asks: {:?}", self.asks);
    }

    pub fn insert_bid(&mut self, node: Node) {
        let order_sig = get_order_signature(node.get_order().order_id, node.get_user_account());
        self.order_sigs.insert(order_sig.clone(), node);
        let directional = DirectionalNode::new(node, self.bid_sort_direction);
        self.bids.push(directional);
    }

    pub fn insert_ask(&mut self, node: Node) {
        let order_sig = get_order_signature(node.get_order().order_id, node.get_user_account());
        self.order_sigs.insert(order_sig.clone(), node);
        let directional = DirectionalNode::new(node, self.ask_sort_direction);
        self.asks.push(directional);
    }

    pub fn get_best_bid(&mut self) -> Option<Node> {
        if let Some(node) = self.bids.pop().map(|node| node.node) {
            let order_sig = get_order_signature(node.get_order().order_id, node.get_user_account());
            if self.order_sigs.contains_key(&order_sig) {
                self.order_sigs.remove(&order_sig);
                return Some(node);
            }
        }
        None
    }

    pub fn get_best_ask(&mut self) -> Option<Node> {
        if let Some(node) = self.asks.pop().map(|node| node.node) {
            let order_sig = get_order_signature(node.get_order().order_id, node.get_user_account());
            if self.order_sigs.contains_key(&order_sig) {
                self.order_sigs.remove(&order_sig);
                return Some(node);
            }
        }
        None
    }

    pub fn get_node(&self, order_sig: &String) -> Option<Node> {
        self.order_sigs.get(order_sig).map(|node| *node)
    }

    pub fn bids_empty(&self) -> bool {
        self.bids.is_empty()
    }

    pub fn asks_empty(&self) -> bool {
        self.asks.is_empty()
    }

    pub fn size(&self) -> usize {
        self.bids.len() + self.asks.len()
    }
}

