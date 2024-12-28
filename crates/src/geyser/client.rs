use crossbeam::channel::Sender;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tokio_stream::StreamExt;
use yellowstone_grpc_proto::prelude::subscribe_update::UpdateOneof;

use crate::geyser::account::{AcctCtx, ToAccount};

use std::fmt::Debug;

use futures::channel::mpsc::SendError;
use futures::Stream;
use futures_util::sink::SinkExt;
use log::*;
use thiserror::Error;
use yellowstone_grpc_client::{GeyserGrpcBuilderError, GeyserGrpcClient, GeyserGrpcClientError};
use yellowstone_grpc_proto::prelude::{SubscribeRequest, SubscribeUpdate};
use yellowstone_grpc_proto::tonic::Status;

use crate::geyser::types::{BlockInfo, ChannelMsg, GeyserConfig, Ix, TxStub};

pub type GeyserClientResult<T = ()> = Result<T, GeyserClientError>;

#[derive(Debug, Error)]
pub enum GeyserClientError {
    #[error("{0}")]
    GeyserBuilder(#[from] GeyserGrpcBuilderError),

    #[error("{0}")]
    GeyserClient(#[from] GeyserGrpcClientError),

    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),

    #[error("{0}")]
    Send(#[from] SendError),

    #[error("{0}")]
    Channel(#[from] crossbeam::channel::SendError<TxStub>),
}


pub struct GeyserClient {
    cfg: GeyserConfig,
}

impl GeyserClient {
    pub fn new(cfg: GeyserConfig) -> Self {
        Self { cfg }
    }

    async fn connect(
        &self,
    ) -> GeyserClientResult<impl Stream<Item = Result<SubscribeUpdate, Status>>> {
        let cfg = self.cfg.clone();
        let x_token: Option<String> = Some(cfg.x_token);
        let mut client = GeyserGrpcClient::build_from_shared(cfg.grpc)?
            .x_token(x_token)?
            .connect()
            .await?;
        let (mut subscribe_tx, stream) = client.subscribe().await?;
        subscribe_tx
            .send(SubscribeRequest::from(self.cfg.clone()))
            .await?;
        Ok(stream)
    }

    pub async fn subscribe(
        &self,
        channel: Sender<ChannelMsg>,
    ) -> anyhow::Result<()> {
        let mut stream = self.connect().await?;
        while let Some(update) = stream.next().await {
            let update = update?;
            if let Some(update) = update.update_oneof {
                match update {
                    UpdateOneof::Transaction(event) => {
                        if let Some(tx_info) = event.transaction {
                            if let Some(tx) = tx_info.transaction {
                                if let Some(msg) = tx.message {
                                    let account_keys: Vec<Pubkey> = msg
                                        .account_keys
                                        .iter()
                                        .flat_map(|k| Pubkey::try_from(k.as_slice()))
                                        .collect();
                                    assert_eq!(account_keys.len(), msg.account_keys.len());

                                    let mut ixs = vec![];
                                    for ix in msg.instructions {
                                        let program: Pubkey = *account_keys
                                            .get(ix.program_id_index as usize)
                                            .ok_or(anyhow::anyhow!(
                                                "Program not found at account key index: {}",
                                                ix.program_id_index
                                            ))?;
                                        let accounts: Vec<Pubkey> = ix
                                            .accounts
                                            .iter()
                                            .flat_map(|ix| {
                                                account_keys.get(*ix as usize).cloned()
                                            })
                                            .collect();
                                        let data = ix.data.clone();

                                        ixs.push(Ix {
                                            program,
                                            accounts,
                                            data,
                                        });
                                    }

                                    let signer =
                                        *account_keys.first().ok_or(anyhow::anyhow!(
                                            "Signer not found at account key index: 0"
                                        ))?;
                                    let signature =
                                        Signature::try_from(tx_info.signature.as_slice())?;
                                    let hash_bytes: [u8; 32] =
                                        msg.recent_blockhash.try_into().map_err(|e| {
                                            anyhow::anyhow!(
                                                "Failed to convert blockhash: {:?}",
                                                e
                                            )
                                        })?;
                                    channel.send(ChannelMsg::Tx(TxStub {
                                        slot: event.slot,
                                        blockhash: Hash::from(hash_bytes).to_string(),
                                        ixs,
                                        signature,
                                        signer,
                                    }))?;
                                }
                            }
                        }
                    }
                    UpdateOneof::Account(event) => {
                        if let Some(account) = event.account {
                            let key = Pubkey::try_from(account.pubkey.as_slice()).map_err(|e| {
                                anyhow::anyhow!("Failed to convert pubkey: {:?}", e)
                            })?;
                            let account = account.to_account()?.clone();
                            channel.send(ChannelMsg::Account(AcctCtx {
                                key,
                                account,
                                slot: event.slot,
                            }))?;
                        }
                    }
                    UpdateOneof::BlockMeta(event) => {
                        if let Some(block_time) = event.block_time {
                            channel.send(ChannelMsg::Block(BlockInfo {
                                slot: event.slot,
                                blockhash: event.blockhash,
                                timestamp: block_time.timestamp,
                            }))?;
                        }
                    }
                    UpdateOneof::Slot(event) => {
                        channel.send(ChannelMsg::Slot(event.slot))?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
