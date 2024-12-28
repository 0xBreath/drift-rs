#![allow(clippy::module_inception)]

use std::{collections::BinaryHeap, str::FromStr, sync::Arc};

use dashmap::{DashMap, DashSet};
use rayon::prelude::*;
use solana_sdk::pubkey::Pubkey;
use rayon::prelude::ParallelBridge;

use crate::{
    dlob::{
        dlob_node::{create_node, get_order_signature, DLOBNode, DirectionalNode, Node, NodeType},
        market::{get_node_subtype_and_type, Exchange, OpenOrders, SubType},
    },
    drift_idl::types::{MarketType, Order, OrderStatus},
    ffi::OraclePriceData,
    math::order::is_resting_limit_order,
    usermap::GlobalUserMap as UserMap,
};
use crate::drift_idl::accounts::User;
use crate::geyser::account::AcctCtx;

#[derive(Clone)]
pub struct DLOB {
    exchange: Exchange,
    _open_orders: OpenOrders,
    _initialized: bool,
    _max_slot_for_resting_limit_orders: Arc<u64>,
}

impl DLOB {
    pub fn new() -> DLOB {
        let exchange = Exchange::new();

        let open_orders = OpenOrders::new();
        open_orders.insert("perp".to_string(), DashSet::new());
        open_orders.insert("spot".to_string(), DashSet::new());

        DLOB {
            exchange,
            _open_orders: open_orders,
            _initialized: true,
            _max_slot_for_resting_limit_orders: Arc::new(0),
        }
    }

    pub fn build_from_usermap(&mut self, usermap: &Arc<DashMap<String, User>>, slot: u64) {
        self.clear();
        // for user_ref in usermap.iter().par_bridge() {
        for user_ref in usermap.iter() {
            let user = user_ref.value();
            let user_key = user_ref.key();
            let ctx: AcctCtx<&User> = AcctCtx {
                key: Pubkey::from_str(user_key).expect("Valid pubkey"),
                account: user,
                slot,
            };
            self.update_user(ctx);
        }
        self._initialized = true;
    }

    pub fn update_user(&mut self, ctx: AcctCtx<&User>)  {
        let AcctCtx {
            key,
            account: user,
            slot
        } = ctx;
        for order in user.orders.iter() {
            if order.status == OrderStatus::Init {
                continue;
            }
            self.insert_order(order, key, slot);
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.exchange.perp_size(), self.exchange.spot_size())
    }

    /// for debugging
    pub fn print_all_spot_orders(&self) {
        for market in self.exchange.spot.iter() {
            println!("market index: {}", market.key());
            market.value().print_all_orders();
        }
    }

    pub fn clear(&mut self) {
        self.exchange.clear();
        self._open_orders.clear();
        self._initialized = false;
        self._max_slot_for_resting_limit_orders = Arc::new(0);
    }

    pub fn insert_order(&self, order: &Order, user_account: Pubkey, slot: u64) {
        let market_type = order.market_type.as_str();
        let market_index = order.market_index;

        let (subtype, node_type) = get_node_subtype_and_type(order, slot);
        let node = create_node(node_type, *order, user_account);

        self.exchange
            .add_market_indempotent(&market_type, market_index);

        let mut market = match order.market_type {
            MarketType::Perp => self.exchange.perp.get_mut(&market_index).expect("market"),
            MarketType::Spot => self.exchange.spot.get_mut(&market_index).expect("market"),
        };

        let order_list = market.get_order_list_for_node_insert(node_type);

        match subtype {
            SubType::Bid => order_list.insert_bid(node),
            SubType::Ask => order_list.insert_ask(node),
            _ => {}
        }
    }

    pub fn get_order(&self, order_id: u32, user_account: Pubkey) -> Option<Order> {
        let order_signature = get_order_signature(order_id, user_account);
        for order_list in self.exchange.get_order_lists() {
            if let Some(node) = order_list.get_node(&order_signature) {
                return Some(*node.get_order());
            }
        }

        None
    }

    fn update_resting_limit_orders_for_market_type(&mut self, slot: u64, market_type: MarketType) {
        let mut new_taking_asks: BinaryHeap<DirectionalNode> = BinaryHeap::new();
        let mut new_taking_bids: BinaryHeap<DirectionalNode> = BinaryHeap::new();

        let market = match market_type {
            MarketType::Perp => &self.exchange.perp,
            MarketType::Spot => &self.exchange.spot,
        };

        for mut market_ref in market.iter_mut() {
            let market = market_ref.value_mut();

            for directional_node in market.taking_limit_orders.bids.iter() {
                if is_resting_limit_order(directional_node.node.get_order(), slot) {
                    market
                        .resting_limit_orders
                        .insert_bid(directional_node.node)
                } else {
                    new_taking_bids.push(*directional_node)
                }
            }

            for directional_node in market.taking_limit_orders.asks.iter() {
                if is_resting_limit_order(directional_node.node.get_order(), slot) {
                    market
                        .resting_limit_orders
                        .insert_ask(directional_node.node);
                } else {
                    new_taking_asks.push(*directional_node);
                }
            }

            market.taking_limit_orders.bids = new_taking_bids.clone();
            market.taking_limit_orders.asks = new_taking_asks.clone();
        }
    }

    pub fn update_resting_limit_orders(&mut self, slot: u64) {
        if slot <= *self._max_slot_for_resting_limit_orders {
            return;
        }

        self._max_slot_for_resting_limit_orders = Arc::new(slot);

        self.update_resting_limit_orders_for_market_type(slot, MarketType::Perp);
        self.update_resting_limit_orders_for_market_type(slot, MarketType::Spot);
    }

    pub fn get_best_orders(
        &self,
        market_type: MarketType,
        sub_type: SubType,
        node_type: NodeType,
        market_index: u16,
    ) -> Vec<Node> {
        let market = match market_type {
            MarketType::Perp => self.exchange.perp.get_mut(&market_index).expect("market"),
            MarketType::Spot => self.exchange.spot.get_mut(&market_index).expect("market"),
        };
        let mut order_list = market.get_order_list_for_node_type(node_type);

        let mut best_orders: Vec<Node> = vec![];

        match sub_type {
            SubType::Bid => {
                while !order_list.bids_empty() {
                    if let Some(node) = order_list.get_best_bid() {
                        best_orders.push(node);
                    }
                }
            }
            SubType::Ask => {
                while !order_list.asks_empty() {
                    if let Some(node) = order_list.get_best_ask() {
                        best_orders.push(node);
                    }
                }
            }
            _ => unimplemented!(),
        }

        best_orders
    }

    pub fn get_resting_limit_asks(
        &mut self,
        slot: u64,
        market_type: MarketType,
        market_index: u16,
        oracle_price_data: OraclePriceData,
    ) -> Vec<Node> {
        self.update_resting_limit_orders(slot);

        let mut resting_limit_orders = self.get_best_orders(
            market_type,
            SubType::Ask,
            NodeType::RestingLimit,
            market_index,
        );
        let mut floating_limit_orders = self.get_best_orders(
            market_type,
            SubType::Ask,
            NodeType::FloatingLimit,
            market_index,
        );

        let comparative = Box::new(
            |node_a: &Node, node_b: &Node, slot: u64, oracle_price_data: OraclePriceData| {
                node_a.get_price(oracle_price_data, slot)
                    > node_b.get_price(oracle_price_data, slot)
            },
        );

        let mut all_orders = vec![];
        all_orders.append(&mut resting_limit_orders);
        all_orders.append(&mut floating_limit_orders);

        all_orders.sort_by(|a, b| {
            if comparative(a, b, slot, oracle_price_data) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        });

        all_orders
    }

    pub fn get_resting_limit_bids(
        &mut self,
        slot: u64,
        market_type: MarketType,
        market_index: u16,
        oracle_price_data: OraclePriceData,
    ) -> Vec<Node> {
        self.update_resting_limit_orders(slot);

        let mut resting_limit_orders = self.get_best_orders(
            market_type,
            SubType::Bid,
            NodeType::RestingLimit,
            market_index,
        );
        let mut floating_limit_orders = self.get_best_orders(
            market_type,
            SubType::Bid,
            NodeType::FloatingLimit,
            market_index,
        );

        let comparative = Box::new(
            |node_a: &Node, node_b: &Node, slot: u64, oracle_price_data: OraclePriceData| {
                node_a.get_price(oracle_price_data, slot)
                    < node_b.get_price(oracle_price_data, slot)
            },
        );

        let mut all_orders = vec![];
        all_orders.append(&mut resting_limit_orders);
        all_orders.append(&mut floating_limit_orders);

        all_orders.sort_by(|a, b| {
            if comparative(a, b, slot, oracle_price_data) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        });

        all_orders
    }
}

impl Default for DLOB {
    fn default() -> Self {
        Self::new()
    }
}
