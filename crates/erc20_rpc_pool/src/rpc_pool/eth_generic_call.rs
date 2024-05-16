use crate::rpc_pool::web3_error_list::check_if_proper_rpc_error;
use crate::rpc_pool::VerifyEndpointResult;
use crate::Web3RpcPool;
use erc20_payment_lib_common::{
    DriverEvent, DriverEventContent, Web3RpcPoolContent, Web3RpcPoolInfo,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use web3::{api::Eth, helpers::CallFuture};

pub trait EthMethod<T: web3::Transport> {
    const METHOD: &'static str;
    type Args: Clone;
    type Return: DeserializeOwned;

    fn do_call(eth: Eth<T>, args: Self::Args) -> CallFuture<Self::Return, T::Out>;
}

impl Web3RpcPool {
    pub async fn eth_generic_call<EthMethodCall: EthMethod<web3::transports::Http>>(
        self: Arc<Self>,
        args: EthMethodCall::Args,
    ) -> Result<EthMethodCall::Return, web3::Error> {
        let mut loop_no = 0;
        const LOOP_COUNT: usize = 4;
        loop {
            let resp = self.clone().choose_best_endpoints().await;
            if resp.allowed_endpoints.is_empty() && !resp.is_resolving {
                log::warn!("No valid endpoints found for chain id {}, wait until next check. Call yagna payment driver rpc --verify for details", self.chain_id);
                return Err(web3::Error::Unreachable);
            }
            let idx_vec = resp.allowed_endpoints;
            if let Some(idx_chosen) = idx_vec.first() {
                self.mark_rpc_chosen(*idx_chosen);
            }

            if idx_vec.is_empty() {
                if loop_no >= LOOP_COUNT {
                    if let Some(event_sender) =
                        self.event_sender.clone().and_then(|es| es.upgrade())
                    {
                        let _ = event_sender
                            .send(DriverEvent {
                                create_date: chrono::Utc::now(),
                                content: DriverEventContent::Web3RpcMessage(Web3RpcPoolInfo {
                                    chain_id: self.chain_id,
                                    content: Web3RpcPoolContent::AllEndpointsFailed,
                                }),
                            })
                            .await;
                    }
                    log::warn!(
                        "Seems like all RPC endpoints failed - chain id: {}",
                        self.chain_id
                    );
                    return Err(web3::Error::Unreachable);
                }
                // sleep for 800, 1200, 2000, 2800 ms - total max sleep time is 6800 ms
                let sleep_times: [u64; LOOP_COUNT] = [800, 1200, 2000, 2800];
                tokio::time::sleep(Duration::from_millis(sleep_times[loop_no])).await;
                loop_no += 1;
                continue;
            }

            for idx in idx_vec {
                let res = match self.get_web3(idx) {
                    Some(web3) => tokio::time::timeout(
                        self.get_max_timeout(idx),
                        EthMethodCall::do_call(web3.eth(), args.clone()),
                    ),
                    None => {
                        //this case is possible if endpoint is removed from pool, just skip it and try next one
                        log::warn!("No web3 instance found on specified index");
                        continue;
                    }
                };

                let err = match res.await {
                    Ok(Ok(balance)) => {
                        self.mark_rpc_success(idx, EthMethodCall::METHOD.to_string());
                        if let Some(event_sender) =
                            self.event_sender.clone().and_then(|es| es.upgrade())
                        {
                            let _ = event_sender
                                .send(DriverEvent {
                                    create_date: chrono::Utc::now(),
                                    content: DriverEventContent::Web3RpcMessage(Web3RpcPoolInfo {
                                        chain_id: self.chain_id,
                                        content: Web3RpcPoolContent::Success,
                                    }),
                                })
                                .await;
                        }
                        return Ok(balance);
                    }
                    Ok(Err(e)) => match e {
                        web3::Error::Rpc(e) => {
                            let proper = check_if_proper_rpc_error(&e.to_string());
                            if proper {
                                self.mark_rpc_success(idx, EthMethodCall::METHOD.to_string());
                                if let Some(event_sender) =
                                    self.event_sender.clone().and_then(|es| es.upgrade())
                                {
                                    let _ = event_sender
                                        .send(DriverEvent {
                                            create_date: chrono::Utc::now(),
                                            content: DriverEventContent::Web3RpcMessage(
                                                Web3RpcPoolInfo {
                                                    chain_id: self.chain_id,
                                                    content: Web3RpcPoolContent::Success,
                                                },
                                            ),
                                        })
                                        .await;
                                }
                                return Err(web3::Error::Rpc(e));
                            } else {
                                log::warn!(
                                    "Unknown RPC error when calling {} from endpoint {}: {}",
                                    EthMethodCall::METHOD,
                                    self.get_name(idx),
                                    e
                                );
                                self.mark_rpc_error(
                                    idx,
                                    EthMethodCall::METHOD.to_string(),
                                    VerifyEndpointResult::RpcWeb3Error(e.to_string()),
                                );
                                web3::Error::Rpc(e)
                            }
                        }
                        _ => {
                            log::warn!(
                                "Error doing call {} from endpoint {}: {}",
                                EthMethodCall::METHOD,
                                self.get_name(idx),
                                e
                            );
                            self.mark_rpc_error(
                                idx,
                                EthMethodCall::METHOD.to_string(),
                                VerifyEndpointResult::OtherNetworkError(e.to_string()),
                            );
                            e
                        }
                    },
                    Err(e) => {
                        log::warn!(
                            "Timeout when getting data from endpoint {}: {}",
                            self.get_name(idx),
                            e
                        );
                        self.mark_rpc_error(
                            idx,
                            EthMethodCall::METHOD.to_string(),
                            VerifyEndpointResult::Unreachable,
                        );
                        web3::Error::Unreachable
                    }
                };
                if loop_no >= LOOP_COUNT {
                    if let Some(event_sender) =
                        self.event_sender.clone().and_then(|es| es.upgrade())
                    {
                        let _ = event_sender
                            .send(DriverEvent {
                                create_date: chrono::Utc::now(),
                                content: DriverEventContent::Web3RpcMessage(Web3RpcPoolInfo {
                                    chain_id: self.chain_id,
                                    content: Web3RpcPoolContent::Error(format!(
                                        "Web3 rpc call failed {}",
                                        err
                                    )),
                                }),
                            })
                            .await;
                    }
                    return Err(err);
                }
                // sleep for 800, 1200, 2000, 2800 ms - total max sleep time is 6800 ms
                let sleep_times: [u64; LOOP_COUNT] = [800, 1200, 2000, 2800];
                tokio::time::sleep(Duration::from_millis(sleep_times[loop_no])).await;
                loop_no += 1;
            }
        }
    }
}
