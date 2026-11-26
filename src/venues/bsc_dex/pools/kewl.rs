//! KEWL. Обычный CREATE из EIP-2535 с отдельными обработчиками регистрации и обмена.
use super::{abi_check::IPoolImmutables, abi_v2v3::IV2Factory, diamond, registry::Entry};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    node::{chain_id, jsonrpc, rpc},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U64, keccak256};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

const FACTORY: &[&str] = &[
    "createPair(address,address)",
    "getPair(address,address)",
    "allPairs(uint256)",
    "allPairsLength()",
];
const ROUTER: &[&str] = &[
    "getAmountsOut(uint256,address[])",
    "getAmountsIn(uint256,address[])",
    "swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256,uint256,address[],address,uint256)",
];
pub(crate) const PIPS: u32 = 3000;

#[derive(Default)]
struct Nonces {
    next: u64,
    derived: HashMap<Address, u64>,
}
enum Proof {
    Found,
    Incomplete,
    Absent,
}
impl Nonces {
    fn prove(&mut self, factory: Address, pool: Address, count: u64, limit: u64) -> Proof {
        self.next = self.next.max(1);
        if !self.derived.contains_key(&pool) {
            let end = count.min(self.next.saturating_add(limit));
            for n in self.next..end {
                self.derived.insert(factory.create(n), n);
            }
            self.next = self.next.max(end);
        }
        match self.derived.get(&pool) {
            Some(n) if *n < count => Proof::Found,
            None if self.next < count => Proof::Incomplete,
            _ => Proof::Absent,
        }
    }
}
type Directory = HashMap<(u64, Address), Nonces>;
static CREATED: LazyLock<Mutex<Directory>> = LazyLock::new(Mutex::default);

fn rate(ctx: &Ctx, id: &str) -> GatewayResult<()> {
    if ctx.dex_text("dex.v2_fee_source", id)? != "fixed"
        || ctx.dex_dec("dex.v2_fee_pct", id)? != rust_decimal::Decimal::new(3, 1)
    {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "Для этого ABI KEWL должна быть задана фиксированная комиссия 0,3%",
        ));
    }
    Ok(())
}

/// На одном каноническом блоке оба направления реестра, принадлежность/токены пары, код и счётчик CREATE.
pub(crate) async fn pair(
    ctx: &Ctx,
    id: &str,
    pool: Address,
    tokens: (Address, Address),
    at: &Value,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Не подтверждены регистрация или исполняемый код пары KEWL",
        )
    };
    let generation = ctx.settings.generation();
    rate(ctx, id)?;
    let chain = chain_id::chain_id(ctx)?;
    let factory = ctx.dex_addr("dex.factory", id)?;
    let hash: B256 = ctx
        .dex_text("dex.pair_runtime_hash", id)?
        .parse()
        .map_err(|_| bad())?;
    if factory.is_zero() || hash.is_zero() || tokens.0.is_zero() || tokens.0 >= tokens.1 {
        return Err(bad());
    }
    let (a, b) = tokens;
    let registry = |a, b| {
        Bytes::from(
            IV2Factory::getPairCall {
                tokenA: a,
                tokenB: b,
            }
            .abi_encode(),
        )
    };
    let view = |to: Address, data: Bytes| ("eth_call", json!([{"to":to,"data":data},at]));
    let ops = [
        ("eth_getCode", json!([pool, at])),
        ("eth_getTransactionCount", json!([factory, at])),
        view(factory, registry(a, b)),
        view(factory, registry(b, a)),
        view(pool, IPoolImmutables::factoryCall {}.abi_encode().into()),
        view(pool, IPoolImmutables::token0Call {}.abi_encode().into()),
        view(pool, IPoolImmutables::token1Call {}.abi_encode().into()),
    ];
    let (_, raw) = tokio::try_join!(
        diamond::observe(ctx, id, false, at, FACTORY, None),
        rpc::batch(ctx, &ops)
    )?;
    let mut raw = raw
        .into_iter()
        .collect::<GatewayResult<Vec<_>>>()?
        .into_iter();
    let code = raw.next().ok_or_else(bad)?;
    let count = raw.next().ok_or_else(bad)?;
    let code: Bytes = jsonrpc::decode(&code, rpc::SOURCE)?;
    let count: U64 = jsonrpc::decode(&count, rpc::SOURCE)?;
    if code.is_empty() || keccak256(&code) != hash || ctx.settings.generation() != generation {
        return Err(bad());
    }
    for wanted in [pool, pool, factory, a, b] {
        let item = raw.next().ok_or_else(bad)?;
        let word: Bytes = jsonrpc::decode(&item, rpc::SOURCE)?;
        if IPoolImmutables::factoryCall::abi_decode_returns(&word).ok() != Some(wanted) {
            return Err(bad());
        }
    }
    // Локальная таблица доказывает лишь вычислимое родство. После reorg n обязательно снова меньше on-chain count.
    let status = CREATED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry((chain, factory))
        .or_default()
        .prove(factory, pool, count.to(), 256);
    match status {
        Proof::Found => Ok(()),
        Proof::Incomplete => Err(ctx.err(
            ErrorCode::StaleData,
            "Продолжается проверка CREATE-счётчика фабрики KEWL",
        )),
        Proof::Absent => Err(bad()),
    }
}
pub(crate) async fn checked(ctx: &Ctx, e: &Entry, router: bool, at: &Value) -> GatewayResult<()> {
    let generation = ctx.settings.generation();
    let id = e.family.id;
    let pool = e
        .pool
        .reference
        .own_contract()
        .ok_or_else(|| ctx.err(ErrorCode::InvalidAddress, "У пары KEWL нет адреса"))?;
    if e.pool.fee_pips.is_some_and(|fee| fee != PIPS) {
        return Err(ctx.err(ErrorCode::StaleData, "Ставка KEWL расходится с кодом пары"));
    }
    let check = pair(ctx, id, pool, (e.pool.token0, e.pool.token1), at);
    if router {
        if ctx.dex_addr("dex.router", id)? != ctx.dex_addr("dex.factory", id)? {
            return Err(ctx.err(
                ErrorCode::InvalidAddress,
                "Роутер и фабрика KEWL находятся на одном прокси",
            ));
        }
        tokio::try_join!(check, diamond::observe(ctx, id, true, at, ROUTER, None))?;
    } else {
        check.await?;
    }
    if ctx.settings.generation() != generation {
        return Err(ctx.err(ErrorCode::StaleData, "Изменилась настройка KEWL"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_progress_shares_derived_addresses_and_checks_reorg_count() {
        let factory = "0x5636a64b835f4e3821c798fda16e0ba106357646"
            .parse::<Address>()
            .unwrap();
        let last = "0x12b7ec8e98326203a2e6fb97885932a191d075a2"
            .parse::<Address>()
            .unwrap();
        let first = "0xf64810a744b3863f81e33112d8b8f4914fe50069"
            .parse::<Address>()
            .unwrap();
        let mut found = Nonces::default();
        assert!(matches!(
            found.prove(factory, last, 5, 2),
            Proof::Incomplete
        ));
        assert!(matches!(found.prove(factory, last, 5, 2), Proof::Found));
        assert!(matches!(found.prove(factory, first, 5, 0), Proof::Found));
        assert!(matches!(found.prove(factory, last, 4, 0), Proof::Absent));
        assert!(matches!(found.prove(factory, last, 5, 0), Proof::Found));
        assert!(matches!(
            found.prove(factory, Address::ZERO, 5, 2),
            Proof::Absent
        ));
    }
}
