mod resolver;
mod verifier;

use crate::rpc_pool::pool::resolver::ExternalSourceResolver;
use crate::rpc_pool::pool::verifier::EndpointsVerifier;
use crate::rpc_pool::verify::{ReqStats, Web3EndpointParams, Web3RpcSingleParams};
use crate::rpc_pool::VerifyEndpointResult;
use crate::Web3RpcInfo;
use chrono::Utc;
use erc20_payment_lib_common::DriverEvent;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use thunderdome::{Arena, Index};
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::TokioAsyncResolver;
use uuid::Uuid;
use web3::transports::Http;
use web3::Web3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3ExternalEndpointList {
    pub chain_id: u64,
    pub names: Vec<String>,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3ExternalJsonSource {
    pub chain_id: u64,
    pub unique_source_id: Uuid,
    pub url: String,
    pub endpoint_params: Web3EndpointParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3ExternalDnsSource {
    pub chain_id: u64,
    pub unique_source_id: Uuid,
    pub dns_url: String,
    pub endpoint_params: Web3EndpointParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3ExternalSources {
    pub json_sources: Vec<Web3ExternalJsonSource>,
    pub dns_sources: Vec<Web3ExternalDnsSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Web3RpcEndpoint {
    #[serde(skip)]
    pub web3: Option<Web3<Http>>,
    pub web3_rpc_params: Web3RpcSingleParams,
    pub web3_rpc_info: Web3RpcInfo,
}

impl Web3RpcEndpoint {
    pub fn get_score(&self) -> f64 {
        if self.is_removed() {
            return 0.0;
        }
        if !self.web3_rpc_info.is_allowed {
            return 0.0;
        }
        let negative_score = self.web3_rpc_info.penalty_from_last_critical_error as f64
            + self.web3_rpc_info.penalty_from_ms as f64
            + self.web3_rpc_info.penalty_from_head_behind as f64
            + self.web3_rpc_info.penalty_from_errors as f64;

        let negative_score_exp = (-negative_score / 200.0).exp();
        //negative_score_exp should be in 0 to 1 range
        negative_score_exp * 75.0 + self.web3_rpc_info.bonus_from_last_chosen as f64
    }
    pub fn get_validation_score(&self) -> f64 {
        if !self.web3_rpc_info.is_allowed {
            return 0.0;
        }
        let negative_score = self.web3_rpc_info.penalty_from_ms as f64
            + self.web3_rpc_info.penalty_from_head_behind as f64;
        let negative_score_exp = (-negative_score / 200.0).exp();

        negative_score_exp * 100.0
    }

    pub fn is_allowed(&self) -> bool {
        if self.web3_rpc_info.removed_date.is_some() {
            return false;
        }
        self.web3_rpc_info.is_allowed || self.web3_rpc_params.web3_endpoint_params.skip_validation
    }

    pub fn is_removed(&self) -> bool {
        self.web3_rpc_info.removed_date.is_some()
    }
}

pub type Web3PoolType = Arc<Mutex<Arena<Arc<RwLock<Web3RpcEndpoint>>>>>;

#[derive(Debug)]
pub struct Web3RpcPool {
    pub chain_id: u64,
    //last_verified: Option<DateTime<Utc>>,
    pub endpoints: Web3PoolType,
    pub verify_mutex: tokio::sync::Mutex<()>,
    pub last_success_endpoints: Arc<Mutex<VecDeque<Index>>>,
    pub event_sender: Option<tokio::sync::mpsc::WeakSender<DriverEvent>>,
    pub external_json_sources: Vec<Web3ExternalJsonSource>,
    pub external_dns_sources: Vec<Web3ExternalDnsSource>,

    pub check_external_sources_interval: Duration,
    pub verify_rpc_min_interval: Duration,

    pub external_sources_resolver: Arc<ExternalSourceResolver>,
    pub endpoint_verifier: Arc<EndpointsVerifier>,
}

pub async fn resolve_txt_record_to_string_array(record: &str) -> std::io::Result<Vec<String>> {
    let resolver: TokioAsyncResolver =
        TokioAsyncResolver::tokio(ResolverConfig::google(), ResolverOpts::default());

    Ok(resolver
        .txt_lookup(record)
        .await?
        .iter()
        .map(|entry| entry.to_string().trim().to_string())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect::<Vec<_>>())
}

pub struct ChooseBestEndpointsResult {
    pub allowed_endpoints: Vec<Index>,
    pub is_resolving: bool,
}

impl Web3RpcPool {
    pub fn new(
        chain_id: u64,
        endpoints: Vec<Web3RpcSingleParams>,
        json_sources: Vec<Web3ExternalJsonSource>,
        dns_sources: Vec<Web3ExternalDnsSource>,
        events: Option<tokio::sync::mpsc::WeakSender<DriverEvent>>,
        verify_rpc_min_interval: Duration,
        external_sources_interval_check: Duration,
    ) -> Arc<Self> {
        let mut web3_endpoints = Arena::new();
        for endpoint_params in endpoints {
            if endpoint_params.chain_id != chain_id {
                log::error!(
                    "Chain id mismatch {} vs {}",
                    endpoint_params.chain_id,
                    chain_id
                );
                continue;
            }
            let http = Http::new(&endpoint_params.endpoint).unwrap();
            let web3 = Web3::new(http);
            let endpoint = Web3RpcEndpoint {
                web3: Some(web3),
                web3_rpc_params: endpoint_params,
                web3_rpc_info: Default::default(),
            };
            log::debug!("Added endpoint {:?}", endpoint);
            web3_endpoints.insert(Arc::new(RwLock::new(endpoint)));
        }

        let methods = [
            "balance",
            "block",
            "allowance",
            "block_number",
            "estimate_gas",
            "logs",
            "send_raw_transaction",
            "transaction",
            "transaction_count",
            "transaction_receipt",
        ];
        for method in methods {
            metrics::counter!("web3_rpc_success", 0, "chain_id" => chain_id.to_string(), "method" => method);
            metrics::counter!("web3_rpc_error", 0, "chain_id" => chain_id.to_string(), "method" => method);
        }

        let s = Arc::new(Self {
            chain_id,
            endpoints: Arc::new(Mutex::new(web3_endpoints)),
            verify_mutex: tokio::sync::Mutex::new(()),
            last_success_endpoints: Arc::new(Mutex::new(VecDeque::new())),
            event_sender: events,
            external_json_sources: json_sources,
            external_dns_sources: dns_sources,
            verify_rpc_min_interval,
            check_external_sources_interval: external_sources_interval_check,
            external_sources_resolver: Arc::new(ExternalSourceResolver::new()),
            endpoint_verifier: Arc::new(Default::default()),
        });

        if !s.external_json_sources.is_empty() || !s.external_dns_sources.is_empty() {
            s.external_sources_resolver
                .clone()
                .start_resolve_if_needed(s.clone(), false);
        }
        s
    }

    pub fn new_from_urls(chain_id: u64, endpoints: Vec<String>) -> Arc<Self> {
        let params = endpoints
            .iter()
            .map(|endpoint| Web3RpcSingleParams {
                chain_id,
                name: endpoint.clone(),
                endpoint: endpoint.clone(),
                web3_endpoint_params: Web3EndpointParams {
                    backup_level: 0,
                    skip_validation: false,
                    max_number_of_consecutive_errors: 5,
                    verify_interval_secs: 120,
                    min_interval_requests_ms: None,
                    max_head_behind_secs: Some(120),
                    max_response_time_ms: 5000,
                },
                source_id: None,
            })
            .collect();
        Self::new(
            chain_id,
            params,
            Vec::new(),
            Vec::new(),
            None,
            Duration::from_secs(10),
            Duration::from_secs(300),
        )
    }

    pub fn add_endpoint(&self, endpoint: Web3RpcSingleParams) {
        let mut endpoints_locked = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        if endpoint.chain_id != self.chain_id {
            log::error!(
                "Chain id mismatch {} vs {}",
                endpoint.chain_id,
                self.chain_id
            );
            return;
        }
        for (_idx, el) in endpoints_locked.iter() {
            let el = el.try_read_for(Duration::from_secs(5)).unwrap();
            if !el.is_removed() && el.web3_rpc_params.endpoint == endpoint.endpoint {
                log::debug!("Endpoint {} already exists", endpoint.endpoint);
                return;
            }
        }
        let http = Http::new(&endpoint.endpoint).unwrap();
        let web3 = Web3::new(http);
        let endpoint = Web3RpcEndpoint {
            web3: Some(web3),
            web3_rpc_params: endpoint,
            web3_rpc_info: Default::default(),
        };
        log::debug!("Added endpoint {:?}", endpoint);
        endpoints_locked.insert(Arc::new(RwLock::new(endpoint)));
    }

    pub fn get_chain_id(self) -> u64 {
        self.chain_id
    }

    pub fn extra_score_from_last_chosen(&self) -> (Option<Index>, i64) {
        let mut extra_score_idx = None;
        let mut extra_score = 0;

        {
            let mut last_success_endpoints = self
                .last_success_endpoints
                .try_lock_for(Duration::from_secs(5))
                .unwrap();
            while last_success_endpoints.len() > 4 {
                last_success_endpoints.pop_back();
            }

            if let Some(last_chosen) = last_success_endpoints.front() {
                extra_score_idx = Some(*last_chosen);
                extra_score += 10;
            }
            if let Some(last_chosen) = last_success_endpoints.get(1) {
                if extra_score_idx == Some(*last_chosen) {
                    extra_score += 7;
                } else {
                    return (extra_score_idx, extra_score);
                }
            }
            if let Some(last_chosen) = last_success_endpoints.get(2) {
                if extra_score_idx == Some(*last_chosen) {
                    extra_score += 5;
                } else {
                    return (extra_score_idx, extra_score);
                }
            }
            if let Some(last_chosen) = last_success_endpoints.get(3) {
                if extra_score_idx == Some(*last_chosen) {
                    extra_score += 3;
                } else {
                    return (extra_score_idx, extra_score);
                }
            }
        }
        (extra_score_idx, extra_score)
    }

    fn cleanup_sources_after_grace_period(&self) {
        let grace_period = chrono::Duration::try_seconds(300).unwrap();
        self.endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .retain(|_idx, el| {
                let can_remove = el
                    .try_read_for(Duration::from_secs(5))
                    .unwrap()
                    .web3_rpc_info
                    .removed_date
                    .map(|removed_date| Utc::now() - removed_date > grace_period)
                    .unwrap_or(false);
                !can_remove
            });
    }

    pub async fn choose_best_endpoints(self: Arc<Self>) -> ChooseBestEndpointsResult {
        let task = self
            .external_sources_resolver
            .clone()
            .start_resolve_if_needed(self.clone(), false);

        let is_resolving = task.is_some();

        let endpoints_copy = self
            .endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .clone();
        let (extra_score_idx, extra_score) = self.extra_score_from_last_chosen();
        for (idx, el) in endpoints_copy.iter() {
            el.try_write_for(Duration::from_secs(10))
                .unwrap()
                .web3_rpc_info
                .bonus_from_last_chosen = if Some(idx) == extra_score_idx {
                extra_score
            } else {
                0
            };
        }

        let mut allowed_endpoints = endpoints_copy
            .iter()
            .filter(|(_idx, element)| {
                element
                    .try_read_for(Duration::from_secs(5))
                    .unwrap()
                    .is_allowed()
            })
            .map(|(idx, _element)| idx)
            .collect::<Vec<Index>>();

        allowed_endpoints.sort_by_key(|idx| {
            (endpoints_copy[*idx]
                .try_read_for(Duration::from_secs(5))
                .unwrap()
                .get_score()
                * 1000.0) as i64
        });
        allowed_endpoints.reverse();

        if !allowed_endpoints.is_empty() {
            //todo change type system to allow that call

            let self_cloned = self.clone();

            self_cloned
                .endpoint_verifier
                .clone()
                .start_verify_if_needed(self.clone(), false);

            ChooseBestEndpointsResult {
                allowed_endpoints,
                is_resolving,
            }
        } else {
            let self_cloned = self.clone();
            self_cloned
                .endpoint_verifier
                .clone()
                .start_verify_if_needed(self.clone(), false);
            //let verify_task = tokio::spawn(self_cloned.endpoint_verifier.verify_unverified_endpoints(self));

            loop {
                let is_finished = self_cloned.endpoint_verifier.is_finished();

                if let Some(el) = endpoints_copy
                    .iter()
                    .filter(|(_idx, element)| {
                        element
                            .try_read_for(Duration::from_secs(5))
                            .unwrap()
                            .is_allowed()
                    })
                    .max_by_key(|(_idx, element)| {
                        (element
                            .try_read_for(Duration::from_secs(5))
                            .unwrap()
                            .get_score()
                            * 1000.0) as i64
                    })
                    .map(|(idx, _element)| idx)
                {
                    return ChooseBestEndpointsResult {
                        allowed_endpoints: vec![el],
                        is_resolving,
                    };
                }

                if is_finished {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            //no endpoint could be selected
            ChooseBestEndpointsResult {
                allowed_endpoints: vec![],
                is_resolving,
            }
        }
    }

    pub fn get_web3(&self, idx: Index) -> Option<Web3<Http>> {
        let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        endpoints.get(idx).map(|el| {
            el.try_read_for(Duration::from_secs(5))
                .unwrap()
                .web3
                .clone()
                .expect("web3 field cannot be None")
        })
    }

    pub fn get_name(&self, idx: Index) -> String {
        let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        if let Some(el) = endpoints.get(idx) {
            el.try_read_for(Duration::from_secs(5))
                .unwrap()
                .web3_rpc_params
                .name
                .clone()
        } else {
            "NoIdx".to_string()
        }
    }

    pub fn get_max_timeout(&self, idx: Index) -> std::time::Duration {
        let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        Duration::from_millis(if let Some(el) = endpoints.get(idx) {
            el.try_read_for(Duration::from_secs(5))
                .unwrap()
                .web3_rpc_params
                .web3_endpoint_params
                .max_response_time_ms
        } else {
            0
        })
    }

    pub fn mark_rpc_chosen(&self, idx: Index) {
        let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        endpoints
            .get(idx)
            .unwrap()
            .try_write_for(Duration::from_secs(5))
            .unwrap()
            .web3_rpc_info
            .last_chosen = Some(Utc::now());
    }

    pub fn mark_rpc_success(&self, idx: Index, method: String) {
        // use read lock before write lock to avoid deadlock
        let params = self
            .endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .get(idx)
            .map(|el| {
                el.try_read_for(Duration::from_secs(5))
                    .unwrap()
                    .web3_rpc_params
                    .clone()
            });

        let Some(params) = params else {
            log::error!("mark_rpc_success - no params found for given index");
            return;
        };
        self.last_success_endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .push_front(idx);

        let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
        let stats = &mut endpoints
            .get(idx)
            .unwrap()
            .try_write_for(Duration::from_secs(5))
            .unwrap()
            .web3_rpc_info;

        let el = if let Some(entry) = stats.web3_rpc_stats.request_stats.get_mut(&method) {
            entry
        } else {
            stats
                .web3_rpc_stats
                .request_stats
                .insert(method.clone(), ReqStats::default());
            if let Some(res) = stats.web3_rpc_stats.request_stats.get_mut(&method) {
                res
            } else {
                log::error!("Error inserting method {}", method);
                return;
            }
        };
        metrics::counter!("web3_rpc_success", 1, "chain_id" => self.chain_id.to_string(), "endpoint" => params.name.clone());
        metrics::counter!("web3_rpc_success", 1, "chain_id" => self.chain_id.to_string(), "method" => method);
        metrics::counter!("web3_rpc_success", 1, "chain_id" => self.chain_id.to_string());
        el.request_succeeded_count += 1;
        el.last_success_request = Some(Utc::now());

        stats.endpoint_consecutive_errors = 0;
        stats.web3_rpc_stats.last_success_request = Some(Utc::now());
        stats.web3_rpc_stats.request_count_total_succeeded += 1;
    }

    pub fn mark_rpc_error(&self, idx: Index, method: String, verify_result: VerifyEndpointResult) {
        // use read lock before write lock to avoid deadlock
        let params = self
            .endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .get(idx)
            .unwrap()
            .try_read_for(Duration::from_secs(5))
            .unwrap()
            .web3_rpc_params
            .clone();

        {
            // lock stats for writing, do not use read lock here
            let endpoints = self.endpoints.try_lock_for(Duration::from_secs(5)).unwrap();
            let stats = &mut endpoints
                .get(idx)
                .unwrap()
                .try_write_for(Duration::from_secs(5))
                .unwrap()
                .web3_rpc_info;
            let el = if let Some(entry) = stats.web3_rpc_stats.request_stats.get_mut(&method) {
                entry
            } else {
                stats
                    .web3_rpc_stats
                    .request_stats
                    .insert(method.clone(), ReqStats::default());
                if let Some(res) = stats.web3_rpc_stats.request_stats.get_mut(&method) {
                    res
                } else {
                    log::error!("Error inserting method {}", method);
                    return;
                }
            };
            el.request_error_count += 1;
            el.last_error_request = Some(Utc::now());

            stats.web3_rpc_stats.last_error_request = Some(Utc::now());
            stats.web3_rpc_stats.request_count_total_error += 1;
            metrics::counter!("web3_rpc_error", 1, "chain_id" => self.chain_id.to_string(), "endpoint" => params.name.clone());
            metrics::counter!("web3_rpc_error", 1, "chain_id" => self.chain_id.to_string(), "method" => method);
            metrics::counter!("web3_rpc_error", 1, "chain_id" => self.chain_id.to_string());
            stats.verify_result = Some(verify_result);
            stats.endpoint_consecutive_errors += 1;
            stats.penalty_from_last_critical_error += 10;
            if stats.endpoint_consecutive_errors
                > params.web3_endpoint_params.max_number_of_consecutive_errors
            {
                //stats.is_allowed = false;
            }
        } // stats lock is released here
    }

    pub fn get_endpoints_info(&self) -> Vec<(Index, Web3RpcSingleParams, Web3RpcInfo)> {
        self.endpoints
            .try_lock_for(Duration::from_secs(5))
            .unwrap()
            .iter()
            .map(|(idx, w)| {
                (
                    idx,
                    w.try_read_for(Duration::from_secs(5))
                        .unwrap()
                        .web3_rpc_params
                        .clone(),
                    w.try_read_for(Duration::from_secs(5))
                        .unwrap()
                        .web3_rpc_info
                        .clone(),
                )
            })
            .collect()
    }
}
