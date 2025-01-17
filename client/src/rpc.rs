// Copyright 2019 Conflux Foundation. All rights reserved.
// Conflux is free software and distributed under GNU General Public License.
// See http://www.gnu.org/licenses/

use jsonrpc_core::{MetaIoHandler, Result as JsonRpcResult};
use jsonrpc_http_server::{
    AccessControlAllowOrigin, DomainsValidation, Server as HttpServer,
    ServerBuilder as HttpServerBuilder,
};
use jsonrpc_tcp_server::{
    MetaExtractor as TpcMetaExtractor, Server as TcpServer,
    ServerBuilder as TcpServerBuilder,
};
use jsonrpc_ws_server::{
    MetaExtractor as WsMetaExtractor, Server as WsServer,
    ServerBuilder as WsServerBuilder,
};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

mod authcodes;
pub mod error_codes;
pub mod extractor;
mod helpers;
mod http_common;
pub mod impls;
pub mod informant;
mod interceptor;
pub mod metadata;
pub mod rpc_apis;
mod traits;
pub mod types;

pub use cfxcore::rpc_errors::{
    BoxFuture as RpcBoxFuture, Error as RpcError, ErrorKind as RpcErrorKind,
    ErrorKind::JsonRpcError as JsonRpcErrorKind, Result as RpcResult,
};

use self::{
    impls::{
        cfx::{CfxHandler, LocalRpcImpl, RpcImpl, TestRpcImpl},
        common::RpcImpl as CommonImpl,
        light::{
            CfxHandler as LightCfxHandler, DebugRpcImpl as LightDebugRpcImpl,
            RpcImpl as LightImpl, TestRpcImpl as LightTestRpcImpl,
        },
        pool::TransactionPoolHandler,
        pos::{PoSInterceptor, PosHandler},
        pubsub::PubSubClient,
        trace::TraceHandler,
    },
    traits::{
        cfx::Cfx,
        debug::LocalRpc,
        eth_space::{eth::Eth, trace::Trace as EthTrace},
        pool::TransactionPool,
        pos::Pos,
        pubsub::PubSub,
        test::TestRpc,
        trace::Trace,
    },
};

pub use self::types::{Block as RpcBlock, Origin};
use crate::{
    configuration::Configuration,
    rpc::{
        error_codes::request_rejected_too_many_request_error,
        impls::{eth::EthHandler, trace::EthTraceHandler},
        interceptor::{RpcInterceptor, RpcProxy},
        rpc_apis::{Api, ApiSet},
    },
};
pub use metadata::Metadata;
use std::collections::HashSet;
use throttling::token_bucket::{ThrottleResult, TokenBucketManager};

#[derive(Debug, PartialEq)]
pub struct TcpConfiguration {
    pub enabled: bool,
    pub address: SocketAddr,
}

impl TcpConfiguration {
    pub fn new(ip: Option<(u8, u8, u8, u8)>, port: Option<u16>) -> Self {
        let ipv4 = match ip {
            Some(ip) => Ipv4Addr::new(ip.0, ip.1, ip.2, ip.3),
            None => Ipv4Addr::new(0, 0, 0, 0),
        };
        TcpConfiguration {
            enabled: port.is_some(),
            address: SocketAddr::V4(SocketAddrV4::new(ipv4, port.unwrap_or(0))),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct HttpConfiguration {
    pub enabled: bool,
    pub address: SocketAddr,
    pub cors_domains: DomainsValidation<AccessControlAllowOrigin>,
    pub keep_alive: bool,
    // If it's Some, we will manually set the number of threads of HTTP RPC
    // server
    pub threads: Option<usize>,
}

impl HttpConfiguration {
    pub fn new(
        ip: Option<(u8, u8, u8, u8)>, port: Option<u16>, cors: Option<String>,
        keep_alive: bool, threads: Option<usize>,
    ) -> Self
    {
        let ipv4 = match ip {
            Some(ip) => Ipv4Addr::new(ip.0, ip.1, ip.2, ip.3),
            None => Ipv4Addr::new(0, 0, 0, 0),
        };
        HttpConfiguration {
            enabled: port.is_some(),
            address: SocketAddr::V4(SocketAddrV4::new(ipv4, port.unwrap_or(0))),
            cors_domains: match cors {
                None => DomainsValidation::Disabled,
                Some(cors_list) => match cors_list.as_str() {
                    "none" => DomainsValidation::Disabled,
                    "all" => DomainsValidation::AllowOnly(vec![
                        AccessControlAllowOrigin::Any,
                    ]),
                    _ => DomainsValidation::AllowOnly(
                        cors_list.split(',').map(Into::into).collect(),
                    ),
                },
            },
            keep_alive,
            threads,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct WsConfiguration {
    pub enabled: bool,
    pub address: SocketAddr,
    pub max_payload_bytes: usize,
}

impl WsConfiguration {
    pub fn new(
        ip: Option<(u8, u8, u8, u8)>, port: Option<u16>,
        max_payload_bytes: usize,
    ) -> Self
    {
        let ipv4 = match ip {
            Some(ip) => Ipv4Addr::new(ip.0, ip.1, ip.2, ip.3),
            None => Ipv4Addr::new(0, 0, 0, 0),
        };
        WsConfiguration {
            enabled: port.is_some(),
            address: SocketAddr::V4(SocketAddrV4::new(ipv4, port.unwrap_or(0))),
            max_payload_bytes,
        }
    }
}

pub fn setup_public_rpc_apis(
    common: Arc<CommonImpl>, rpc: Arc<RpcImpl>, pubsub: PubSubClient,
    conf: &Configuration,
) -> MetaIoHandler<Metadata>
{
    setup_rpc_apis(
        common,
        rpc,
        pubsub,
        &conf.raw_conf.throttling_conf,
        "rpc",
        conf.raw_conf.public_rpc_apis.list_apis(),
    )
}

pub fn setup_public_eth_rpc_apis(
    common: Arc<CommonImpl>, rpc: Arc<RpcImpl>, pubsub: PubSubClient,
    conf: &Configuration,
) -> MetaIoHandler<Metadata>
{
    setup_rpc_apis(
        common,
        rpc,
        pubsub,
        &conf.raw_conf.throttling_conf,
        "rpc",
        conf.raw_conf.public_evm_rpc_apis.list_apis(),
    )
}

pub fn setup_debug_rpc_apis(
    common: Arc<CommonImpl>, rpc: Arc<RpcImpl>, pubsub: PubSubClient,
    conf: &Configuration,
) -> MetaIoHandler<Metadata>
{
    setup_rpc_apis(
        common,
        rpc,
        pubsub,
        &conf.raw_conf.throttling_conf,
        "rpc_local",
        ApiSet::All.list_apis(),
    )
}

fn setup_rpc_apis(
    common: Arc<CommonImpl>, rpc: Arc<RpcImpl>, pubsub: PubSubClient,
    throttling_conf: &Option<String>, throttling_section: &str,
    apis: HashSet<Api>,
) -> MetaIoHandler<Metadata>
{
    let mut handler = MetaIoHandler::default();
    for api in apis {
        match api {
            Api::Cfx => {
                let cfx =
                    CfxHandler::new(common.clone(), rpc.clone()).to_delegate();
                let interceptor = ThrottleInterceptor::new(
                    throttling_conf,
                    throttling_section,
                );
                handler.extend_with(RpcProxy::new(cfx, interceptor));
            }
            Api::Eth => {
                info!("Add EVM RPC");
                let evm = EthHandler::new(
                    rpc.config.clone(),
                    rpc.consensus.clone(),
                    rpc.sync.clone(),
                    rpc.tx_pool.clone(),
                )
                .to_delegate();
                let evm_trace_handler = EthTraceHandler {
                    trace_handler: TraceHandler::new(
                        rpc.consensus.get_data_manager().clone(),
                        *rpc.sync.network.get_network_type(),
                        rpc.consensus.clone(),
                    ),
                }
                .to_delegate();
                let interceptor = ThrottleInterceptor::new(
                    throttling_conf,
                    throttling_section,
                );
                handler.extend_with(RpcProxy::new(evm, interceptor));
                // TODO(lpl): Set this separately.
                handler.extend_with(evm_trace_handler);
            }
            Api::Debug => {
                handler.extend_with(
                    LocalRpcImpl::new(common.clone(), rpc.clone())
                        .to_delegate(),
                );
            }
            Api::Pubsub => handler.extend_with(pubsub.clone().to_delegate()),
            Api::Test => {
                handler.extend_with(
                    TestRpcImpl::new(common.clone(), rpc.clone()).to_delegate(),
                );
            }
            Api::Trace => {
                let trace = TraceHandler::new(
                    rpc.consensus.get_data_manager().clone(),
                    *rpc.sync.network.get_network_type(),
                    rpc.consensus.clone(),
                )
                .to_delegate();
                let interceptor = ThrottleInterceptor::new(
                    throttling_conf,
                    throttling_section,
                );
                handler.extend_with(RpcProxy::new(trace, interceptor));
            }
            Api::TxPool => {
                let txpool =
                    TransactionPoolHandler::new(common.clone()).to_delegate();
                handler.extend_with(txpool);
            }
            Api::Pos => {
                let pos = PosHandler::new(
                    common.pos_handler.clone(),
                    rpc.consensus.get_data_manager().clone(),
                    *rpc.sync.network.get_network_type(),
                )
                .to_delegate();
                let pos_interceptor =
                    PoSInterceptor::new(common.pos_handler.clone());
                handler.extend_with(RpcProxy::new(pos, pos_interceptor));
            }
        }
    }
    handler
}

pub fn setup_public_rpc_apis_light(
    common: Arc<CommonImpl>, rpc: Arc<LightImpl>, pubsub: PubSubClient,
    conf: &Configuration,
) -> MetaIoHandler<Metadata>
{
    setup_rpc_apis_light(
        common,
        rpc,
        pubsub,
        &conf.raw_conf.throttling_conf,
        "rpc",
        conf.raw_conf.public_rpc_apis.list_apis(),
    )
}

pub fn setup_debug_rpc_apis_light(
    common: Arc<CommonImpl>, rpc: Arc<LightImpl>, pubsub: PubSubClient,
    conf: &Configuration,
) -> MetaIoHandler<Metadata>
{
    let mut light_debug_apis = ApiSet::All.list_apis();
    light_debug_apis.remove(&Api::Trace);
    setup_rpc_apis_light(
        common,
        rpc,
        pubsub,
        &conf.raw_conf.throttling_conf,
        "rpc_local",
        light_debug_apis,
    )
}

fn setup_rpc_apis_light(
    common: Arc<CommonImpl>, rpc: Arc<LightImpl>, pubsub: PubSubClient,
    throttling_conf: &Option<String>, throttling_section: &str,
    apis: HashSet<Api>,
) -> MetaIoHandler<Metadata>
{
    let mut handler = MetaIoHandler::default();
    for api in apis {
        match api {
            Api::Cfx => {
                let cfx = LightCfxHandler::new(common.clone(), rpc.clone())
                    .to_delegate();
                let interceptor = ThrottleInterceptor::new(
                    throttling_conf,
                    throttling_section,
                );
                handler.extend_with(RpcProxy::new(cfx, interceptor));
            }
            Api::Eth => {
                warn!("Light nodes do not support evm ports.");
            }
            Api::Debug => {
                handler.extend_with(
                    LightDebugRpcImpl::new(common.clone(), rpc.clone())
                        .to_delegate(),
                );
            }
            Api::Pubsub => handler.extend_with(pubsub.clone().to_delegate()),
            Api::Test => {
                handler.extend_with(
                    LightTestRpcImpl::new(common.clone(), rpc.clone())
                        .to_delegate(),
                );
            }
            Api::Trace => {
                warn!("Light nodes do not support trace RPC");
            }
            Api::TxPool => {
                warn!("Light nodes do not support txpool RPC");
            }
            Api::Pos => {
                warn!("Light nodes do not support PoS RPC");
            }
        }
    }
    handler
}

pub fn start_tcp<H, T>(
    conf: TcpConfiguration, handler: H, extractor: T,
) -> Result<Option<TcpServer>, String>
where
    H: Into<MetaIoHandler<Metadata>>,
    T: TpcMetaExtractor<Metadata> + 'static,
{
    if !conf.enabled {
        return Ok(None);
    }

    match TcpServerBuilder::with_meta_extractor(handler, extractor)
        .start(&conf.address)
    {
        Ok(server) => Ok(Some(server)),
        Err(io_error) => {
            Err(format!("TCP error: {} (addr = {})", io_error, conf.address))
        }
    }
}

pub fn start_http(
    conf: HttpConfiguration, handler: MetaIoHandler<Metadata>,
) -> Result<Option<HttpServer>, String> {
    if !conf.enabled {
        return Ok(None);
    }
    let mut builder = HttpServerBuilder::new(handler);
    if let Some(threads) = conf.threads {
        builder = builder.threads(threads);
    }

    match builder
        .keep_alive(conf.keep_alive)
        .cors(conf.cors_domains.clone())
        .start_http(&conf.address)
    {
        Ok(server) => Ok(Some(server)),
        Err(io_error) => Err(format!(
            "HTTP error: {} (addr = {})",
            io_error, conf.address
        )),
    }
}

pub fn start_ws<H, T>(
    conf: WsConfiguration, handler: H, extractor: T,
) -> Result<Option<WsServer>, String>
where
    H: Into<MetaIoHandler<Metadata>>,
    T: WsMetaExtractor<Metadata> + 'static,
{
    if !conf.enabled {
        return Ok(None);
    }

    match WsServerBuilder::with_meta_extractor(handler, extractor)
        .max_payload(conf.max_payload_bytes)
        .start(&conf.address)
    {
        Ok(server) => Ok(Some(server)),
        Err(io_error) => {
            Err(format!("WS error: {} (addr = {})", io_error, conf.address))
        }
    }
}

struct ThrottleInterceptor {
    manager: TokenBucketManager,
}

impl ThrottleInterceptor {
    fn new(file: &Option<String>, section: &str) -> Self {
        let manager = match file {
            Some(file) => TokenBucketManager::load(file, Some(section))
                .expect("invalid throttling configuration file"),
            None => TokenBucketManager::default(),
        };

        ThrottleInterceptor { manager }
    }
}

impl RpcInterceptor for ThrottleInterceptor {
    fn before(&self, name: &String) -> JsonRpcResult<()> {
        let bucket = match self.manager.get(name) {
            Some(bucket) => bucket,
            None => return Ok(()),
        };

        let result = bucket.lock().throttle_default();

        match result {
            ThrottleResult::Success => Ok(()),
            ThrottleResult::Throttled(wait_time) => {
                debug!("RPC {} throttled in {:?}", name, wait_time);
                bail!(request_rejected_too_many_request_error(Some(format!(
                    "throttled in {:?}",
                    wait_time
                ))))
            }
            ThrottleResult::AlreadyThrottled => {
                debug!("RPC {} already throttled", name);
                bail!(request_rejected_too_many_request_error(Some(
                    "already throttled, please try again later".into()
                )))
            }
        }
    }
}
