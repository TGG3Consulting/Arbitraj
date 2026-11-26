//! Общие для рынков EtherVista поля фактического роутера: одно чтение на активный профиль и канонический блок.
use super::{families::Family, nami_batch as batch};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;

sol! {
    interface IVista{
        function router() external view returns(address);
        function factory() external view returns(address);
        function WETH() external view returns(address);
        function INIT_CODE_HASH() external view returns(bytes32);
        function getPair(address tokenA,address tokenB) external view returns(address);
        function getReserves() external view returns(uint112 r0,uint112 r1,uint32 ts);
        function balanceOf(address account) external view returns(uint256);
        function buyTotalFee() external view returns(uint8);
        function sellTotalFee() external view returns(uint8);
        function usdcToEth(uint256 units) external view returns(uint256);
        function weth() external view returns(address);
        function owner() external view returns(address);
        function feeReserve() external view returns(uint256);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Profile {
    at: B256,
    factory: Address,
    router: Address,
    wrapped: Address,
    factory_hash: B256,
    router_hash: B256,
}

#[derive(Clone, Copy)]
pub(super) struct Deployment {
    pub factory: Address,
    pub router: Address,
    pub wrapped: Address,
    pub init: B256,
    pub router_weth: U256,
}

type Cell = Arc<OnceCell<Deployment>>;
static CHECKS: LazyLock<Mutex<VecDeque<(Profile, Cell)>>> = LazyLock::new(Mutex::default);

fn cell(profile: Profile) -> Cell {
    let mut checks = CHECKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, found)) = checks.iter().find(|(p, _)| *p == profile) {
        return found.clone();
    }
    if checks.len() >= 16 {
        checks.pop_front();
    }
    let fresh = Cell::default();
    checks.push_back((profile, fresh.clone()));
    fresh
}

async fn read(ctx: &Ctx, p: Profile, tag: &Value) -> GatewayResult<Deployment> {
    let balance = IVista::balanceOfCall { account: p.router };
    let [
        factory_code,
        router_code,
        active,
        bound,
        wrapped,
        init,
        dust,
    ] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([p.factory, tag])),
            ("eth_getCode", json!([p.router, tag])),
            batch::view(tag, p.factory, IVista::routerCall {}),
            batch::view(tag, p.router, IVista::factoryCall {}),
            batch::view(tag, p.router, IVista::WETHCall {}),
            batch::view(tag, p.factory, IVista::INIT_CODE_HASHCall {}),
            batch::view(tag, p.wrapped, balance.clone()),
        ],
    )
    .await?;
    if factory_code.is_empty()
        || router_code.is_empty()
        || keccak256(&factory_code) != p.factory_hash
        || keccak256(&router_code) != p.router_hash
        || batch::decode(&active, IVista::routerCall {})? != p.router
        || batch::decode(&bound, IVista::factoryCall {})? != p.factory
        || batch::decode(&wrapped, IVista::WETHCall {})? != p.wrapped
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "EtherVista: фабрика не авторизует выбранный роутер либо его размещение изменилось",
        ));
    }
    Ok(Deployment {
        factory: p.factory,
        router: p.router,
        wrapped: p.wrapped,
        init: batch::decode(&init, IVista::INIT_CODE_HASHCall {})?,
        router_weth: batch::decode(&dust, balance)?,
    })
}

pub(super) async fn deployed(ctx: &Ctx, f: &Family, tag: &Value) -> GatewayResult<Deployment> {
    let bad = || {
        ctx.err(ErrorCode::NotConfigured,"EtherVista: нужны канонический блок и подтверждённые коды фабрики и действующего роутера")
    };
    let hash = |key: &str| {
        ctx.dex_text(key, f.id)?
            .parse::<B256>()
            .ok()
            .filter(|v| *v != B256::ZERO && *v != keccak256([]))
            .ok_or_else(bad)
    };
    let p = Profile {
        at: tag["blockHash"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(bad)?,
        factory: ctx.dex_addr("dex.factory", f.id)?,
        router: ctx.dex_addr("dex.quoter", f.id)?,
        wrapped: ctx.addr("dex.native_wrapped")?,
        factory_hash: hash("dex.factory_runtime_hash")?,
        router_hash: hash("dex.router_runtime_hash")?,
    };
    if tag["requireCanonical"] != true
        || p.router == p.factory
        || p.wrapped == p.factory
        || p.wrapped == p.router
    {
        return Err(bad());
    }
    cell(p).get_or_try_init(|| read(ctx, p, tag)).await.copied()
}
