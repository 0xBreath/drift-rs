use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use anchor_lang::AccountDeserialize;
use dashmap::DashMap;
use solana_account_decoder::UiAccountEncoding;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::RpcFilterType;
use solana_sdk::commitment_config::CommitmentConfig;
use crate::drift_idl::accounts::{AccountType, User};
use crate::geyser::types::{ChannelMsg, GeyserConfig};
use yellowstone_grpc_proto::prelude::{
    CommitmentLevel, SubscribeRequestFilterAccounts, SubscribeRequestFilterSlots,
};
use crate::dlob::dlob::DLOB;
use crate::dlob::dlob_node::Node;
use crate::geyser::account::{AcctCtx, KeyedAccount};
use crate::geyser::client::GeyserClient;
use crate::drift_idl::decode::Decode;
use crate::ffi::OraclePriceData;
use crate::types::MarketId;

#[derive(Default)]
struct SlotCache {
    slot: Arc<AtomicU64>,
}
impl SlotCache {
    pub fn get(&self) -> u64 {
        self.slot.load(Ordering::Relaxed)
    }
    pub fn set(&self, slot: u64) {
        self.slot.store(slot, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct UserCache {
    users: Arc<DashMap<String, User>>,
    latest_slot: Arc<AtomicU64>,
}
impl UserCache {
    pub fn get_user(&self, key: &str) -> Option<User> {
        self.users.get(key).map(|u| u.value().clone())
    }
    pub fn insert_user(&self, key: String, user: User) {
        self.users.insert(key, user);
    }
    pub fn slot(&self) -> u64 {
        self.latest_slot.load(Ordering::Relaxed)
    }
}

pub struct L3 {
    pub bids: Vec<Node>,
    pub asks: Vec<Node>,
    pub slot: u64,
}
impl L3 {
    pub fn best_bid(&self) -> Option<&Node> {
        self.bids.first()
    }
    pub fn best_ask(&self) -> Option<&Node> {
        self.asks.first()
    }
}

pub struct OrderBook {
    rpc: RpcClient,
    geyser: GeyserClient,
    slot: SlotCache,
    user_cache: UserCache,
    dlob: DLOB
}

impl OrderBook {
    pub fn new(rpc: RpcClient, grpc: String, x_token: String) -> Self {
        Self {
            rpc,
            geyser: GeyserClient::new(Self::orderbook_geyser_config(grpc, x_token)),
            slot: SlotCache::default(),
            user_cache: UserCache::default(),
            dlob: DLOB::new()
        }
    }
    
    fn orderbook_geyser_config(grpc: String, x_token: String) -> GeyserConfig {
        GeyserConfig {
            grpc,
            x_token,
            slots: Some(SubscribeRequestFilterSlots {
                filter_by_commitment: Some(true),
            }),
            accounts: Some(SubscribeRequestFilterAccounts {
                account: vec![],
                owner: vec![crate::constants::ids::drift::ID.to_string()],
                filters: vec![],
            }),
            transactions: None,
            blocks_meta: None,
            commitment: CommitmentLevel::Processed,
        }
    }
    
    async fn get_program_accounts<T: AccountDeserialize>(&self, filters: Vec<RpcFilterType>) -> anyhow::Result<Vec<KeyedAccount<T>>> {
        let account_config = RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            commitment: Some(CommitmentConfig::confirmed()),
            ..Default::default()
        };
        let keyed_accounts = self
            .rpc
            .get_program_accounts_with_config(
                &crate::constants::ids::drift::ID,
                RpcProgramAccountsConfig {
                    filters: Some(filters),
                    account_config,
                    ..Default::default()
                },
            )
            .await?;
        let v: Vec<KeyedAccount<T>> = keyed_accounts
            .into_iter()
            .flat_map(|(key, a)| Result::<_, anyhow::Error>::Ok(KeyedAccount {
                key,
                decoded: T::try_deserialize(&mut a.data.as_slice())?
            }))
            .collect();
        Ok(v)
    }
    
    pub async fn subscribe(&mut self) -> anyhow::Result<()> {
        let (tx, rx) = crossbeam::channel::unbounded::<ChannelMsg>();
        let users = self.get_program_accounts::<User>(vec![crate::memcmp::get_user_filter()]).await?;
        let slot = self.rpc.get_slot().await?;
        self.slot.set(slot);
        for user in users {
            self.user_cache.insert_user(user.key.to_string(), user.decoded);
        }
        self.dlob.build_from_usermap(&self.user_cache.users, slot);
        self.geyser.subscribe(tx).await?;
        
        while let Ok(msg) = rx.recv() {
            match msg {
                ChannelMsg::Slot(slot) => {
                    if slot > self.slot.get() {
                        self.slot.set(slot);
                    }
                }
                ChannelMsg::Account(AcctCtx {
                    key,
                    account,
                    ..
                }) => {
                    if account.owner == crate::constants::ids::drift::ID {
                        let acct = AccountType::decode(account.data.as_slice())
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to decode account: {:?}", e)
                            })?;
                        if let AccountType::User(user) = acct {
                            self.user_cache.insert_user(key.to_string(), user);
                            let ctx: AcctCtx<&User> = AcctCtx {
                                key,
                                account: &user,
                                slot: self.slot.get(),
                            };
                            self.dlob.update_user(ctx);
                        }
                    }
                },
                _ => ()
            }
        }
        Ok(())
    }
    
    pub fn l3(&mut self, market: MarketId, oracle: OraclePriceData) -> L3 {
        let slot = self.slot.get();
        let bids = self.dlob.get_resting_limit_bids(slot, market.kind(), market.index(), oracle);
        let asks = self.dlob.get_resting_limit_asks(slot, market.kind(), market.index(), oracle);
        L3 { bids, asks, slot }
    }
}

#[cfg(test)]
mod tests {
    use solana_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::signature::Keypair;
    use crate::DriftClient;
    use crate::geyser::dlob::OrderBook;
    use crate::types::{Context, MarketId};
    use crate::utils::test_envs::mainnet_endpoint;

    #[tokio::test]
    async fn test_orderbook() -> anyhow::Result<()> {
        let client = DriftClient::new(Context::MainNet, RpcClient::new(mainnet_endpoint()), Keypair::new().into()).await?;
        let sol_perp = MarketId::perp(0);
        client.subscribe_markets(&[sol_perp]).await?;
        client.subscribe_oracles(&[sol_perp]).await?;
        
        let grpc = std::env::var("GRPC").expect("GRPC in env");
        let x_token = std::env::var("X_TOKEN").expect("X_TOKEN in env");
        let mut orderbook = OrderBook::new(RpcClient::new(mainnet_endpoint()), grpc, x_token);
        orderbook.subscribe().await?;
        
        let sol_perp_oracle = client.get_oracle_price_data_and_slot(sol_perp).await?;
        let l3 = orderbook.l3(sol_perp, sol_perp_oracle.data);
        println!("best bid: {:#?}", l3.best_bid());
        println!("best ask: {:#?}", l3.best_ask());
        
        Ok(())
    }
}

