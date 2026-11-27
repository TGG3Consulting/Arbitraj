//! Котировка конкретного строителя и симуляция оплаченного обмена bopAMM в целевом слоте.
//! https://buildernet.org/docs/api#state-stream и https://github.com/gattaca-com/propamm
use super::{
    Builder, Ctx, Deployment, Frame, Head, Preview, Request, builder_snapshot, canonical,
    decimalsCall, rpc, slot,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    settings::scope::Scope,
};
use alloy::{
    primitives::{Address, B256, Bytes, U64, U256, keccak256},
    sol_types::{SolCall, SolValue},
};
use chrono::Utc;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

alloy::sol! {
    function market() external view returns(address);
    function pricing() external view returns(address);
    function usdc() external view returns(address);
    function quote(address input,address output,uint256 amount) external view returns(uint256);
    struct Order {bytes32 leg; bytes32 parent; uint64 height; uint64 timestamp;
                  address input; address output; uint256 amount; uint256 minimum;}
    function execute(Order order) external returns(uint256 spent,uint256 received);
}

#[derive(Clone, Copy)]
struct Role {
    address: Address,
    hash: B256,
}
struct Profile {
    market: Role,
    pricing: Role,
    registry: Role,
    module: Option<Role>,
}
impl Profile {
    fn parse(rows: &Value) -> Option<Self> {
        let mut roles = BTreeMap::new();
        let mut accounts = BTreeSet::new();
        for row in rows.as_array()? {
            let mut fields = row.as_str()?.split_whitespace();
            let role = fields.next()?;
            let address: Address = fields.next()?.parse().ok()?;
            let hash: B256 = fields.next()?.parse().ok()?;
            if !matches!(role, "market" | "pricing" | "registry" | "module")
                || fields.next().is_some()
                || address.is_zero()
                || hash.is_zero()
                || !accounts.insert(address)
                || roles.insert(role, Role { address, hash }).is_some()
            {
                return None;
            }
        }
        Some(Self {
            market: roles.remove("market")?,
            pricing: roles.remove("pricing")?,
            registry: roles.remove("registry")?,
            module: roles.remove("module"),
        })
    }
    async fn verify(&self, ctx: &Ctx, h: B256, firm: Option<Role>) -> GatewayResult<()> {
        let verify = async |r: Role| {
            let code: Bytes = rpc::call(
                ctx,
                "eth_getCode",
                json!([r.address,{"blockHash":h,"requireCanonical":true}]),
            )
            .await?;
            if code.is_empty() || keccak256(code) != r.hash {
                return Err(ctx.err(
                    ErrorCode::NotApplicable,
                    "bopAMM: изменился код контракта из профиля",
                ));
            }
            Ok(())
        };
        let (_, _, _, pricing, cash) = tokio::try_join!(
            verify(self.market),
            verify(self.pricing),
            verify(self.registry),
            canonical(ctx, h, self.market.address, pricingCall {}),
            canonical(ctx, h, self.market.address, usdcCall {})
        )?;
        if pricing != self.pricing.address || cash.is_zero() {
            return Err(ctx.err(
                ErrorCode::NotApplicable,
                "bopAMM: рынок привязан к другому источнику цен",
            ));
        }
        if let Some(r) = firm {
            let (_, m, p, c) = tokio::try_join!(
                verify(r),
                canonical(ctx, h, r.address, marketCall {}),
                canonical(ctx, h, r.address, pricingCall {}),
                canonical(ctx, h, r.address, usdcCall {})
            )?;
            if (m, p, c) != (self.market.address, pricing, cash) {
                return Err(ctx.err(
                    ErrorCode::NotApplicable,
                    "bopAMM: модуль кошелька привязан к другому рынку",
                ));
            }
        }
        Ok(())
    }
}

fn frame(builder: Builder, result: &Value, p: &Profile) -> Option<Frame> {
    let accepts = |s: &str| {
        s.parse::<Address>()
            .ok()
            .is_some_and(|a| a == p.market.address || a == p.pricing.address)
    };
    let (row, header) = match builder {
        Builder::Titan => {
            let mut matches = result["venues"].as_array()?.iter().filter(|r| {
                r["maker"] == "Bebop" && r["quoteAddress"].as_str().is_some_and(accepts)
            });
            let row = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            (row, row)
        }
        Builder::BuilderNet => {
            let mut matches = result.as_object()?.iter().filter(|(k, _)| accepts(k));
            let (_, row) = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            (row, result)
        }
    };
    let target = header["blockNumber"].as_u64().or_else(|| {
        serde_json::from_value::<U64>(header["blockNumber"].clone())
            .ok()
            .map(|v| v.to::<u64>())
    })?;
    let slot = header["slot"].as_u64()?;
    // Не переносим в симуляцию баланс, nonce, код или записи чужого токена с публичного сервера.
    if row["stateOverride"].as_object()?.iter().any(|(a, v)| {
        a.parse::<Address>().ok() != Some(p.registry.address)
            && v.get("stateDiff")
                .and_then(Value::as_object)
                .is_some_and(|d| !d.is_empty())
    }) {
        return None;
    }
    Frame::from_row(
        row,
        target,
        slot,
        Deployment {
            router: p.market.address,
            engine: p.registry.address,
            runtime: p.market.hash,
        },
    )
}

pub(in crate::venues::bsc_dex) async fn preview(ctx: &Ctx, r: Request) -> GatewayResult<Preview> {
    let bad = |reason| ctx.err(ErrorCode::NotApplicable, reason);
    if ctx.chain_id() != Some(1) || r.input.is_zero() || r.output.is_zero() || r.input == r.output {
        return Err(bad("bopAMM: нужны Ethereum и два разных ERC20"));
    }
    let rev = ctx.settings.generation();
    let p = ctx
        .settings
        .get_exact("bopamm.roles", &Scope::Venue(ctx.place.id.to_string()))
        .as_ref()
        .and_then(Profile::parse)
        .ok_or_else(|| bad("bopAMM: укажите адреса и хеши рынка, оракула и реестра"))?;
    let builder = r
        .builder
        .as_deref()
        .and_then(Builder::parse)
        .ok_or_else(|| bad("bopAMM: укажите titan или buildernet"))?;
    let payer = r
        .trader
        .filter(|a| !a.is_zero())
        .ok_or_else(|| bad("bopAMM: укажите адрес плательщика"))?;
    let firm = if let Some(t) = &r.firm {
        let owner = super::super::wallet::signer::address(&ctx.with_account(&t.wallet))?;
        let floor =
            super::super::num::exact_amount(Some(&t.minimum_raw), None, 0).ok_or_else(|| {
                bad("bopAMM: укажите положительный минимум в атомах выходного токена")
            })?;
        if payer != owner || t.fee_tenths_bps.is_some() {
            return Err(bad("bopAMM: неверный плательщик или плата"));
        }
        Some((
            p.module
                .ok_or_else(|| bad("bopAMM: укажите размещённый модуль и его хеш"))?,
            floor,
        ))
    } else {
        None
    };
    if r.market.is_some_and(|a| a != p.market.address) {
        return Err(bad("bopAMM: указан другой рынок"));
    }
    let (chain, h, snapshot): (U64, Head, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        builder_snapshot(ctx, builder)
    )?;
    let stale = || {
        ctx.err(
            ErrorCode::StaleData,
            "bopAMM: кадр, канонический родитель или настройки сменились",
        )
    };
    let f = frame(builder, &snapshot, &p).ok_or_else(stale)?;
    let current = || {
        f.current(
            h.number.to(),
            h.timestamp.to(),
            Utc::now().timestamp().max(0) as u64,
        )
    };
    if chain != U64::from(1) || !current() {
        return Err(stale());
    }
    let (_, precision) = tokio::try_join!(
        p.verify(ctx, h.hash, firm.map(|x| x.0)),
        canonical(ctx, h.hash, r.input, decimalsCall {})
    )?;
    let amount = super::super::num::exact_amount(
        r.amount_raw.as_deref(),
        r.amount_tokens.as_deref(),
        precision,
    )
    .ok_or_else(|| bad("bopAMM: недопустимая точная сумма входа"))?;
    let (to, data) = if let Some((m, minimum)) = firm {
        let leg = keccak256((h.hash, payer, r.input, r.output, amount, minimum).abi_encode());
        (
            m.address,
            executeCall {
                order: Order {
                    leg,
                    parent: h.hash,
                    height: f.target,
                    timestamp: f.timestamp,
                    input: r.input,
                    output: r.output,
                    amount,
                    minimum,
                },
            }
            .abi_encode(),
        )
    } else {
        (
            p.market.address,
            quoteCall {
                input: r.input,
                output: r.output,
                amount,
            }
            .abi_encode(),
        )
    };
    let rows = slot::run(
        ctx,
        &h,
        &f,
        json!([{"from":payer,"to":to,"data":Bytes::from(data)}]),
    )
    .await?;
    let failure = || {
        ctx.err(
            ErrorCode::NoRoute,
            "bopAMM: этот кадр не подтвердил выплату для указанного объёма",
        )
    };
    let [one] = rows.as_slice() else {
        return Err(failure());
    };
    if one.status != U64::from(1) {
        return Err(failure());
    }
    let net = if let Some((_, minimum)) = firm {
        let fill = executeCall::abi_decode_returns(&one.return_data).map_err(|_| failure())?;
        if fill.spent != amount || fill.received < minimum || one.gas_used.is_zero() {
            return Err(failure());
        }
        fill.received
    } else {
        quoteCall::abi_decode_returns(&one.return_data).map_err(|_| failure())?
    };
    if net.is_zero() {
        return Err(failure());
    }
    let reconfirm = async {
        if firm.is_some() {
            let latest = builder_snapshot(ctx, builder).await?;
            let fresh = frame(builder, &latest, &p).ok_or_else(stale)?;
            if (fresh.target, fresh.slot, &fresh.diff) != (f.target, f.slot, &f.diff) {
                return Err(stale());
            }
        }
        Ok(())
    };
    let (latest, ()): (Head, ()) = tokio::try_join!(
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        reconfirm
    )?;
    if latest.hash != h.hash || !current() || rev != ctx.settings.generation() {
        return Err(stale());
    }
    Ok(Preview {
        source: "bopamm".into(),
        input: r.input,
        output: r.output,
        spent_raw: amount.to_string(),
        received_raw: net.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: h.hash.to_string(),
        block: Some(h.number.to()),
        expires: Some(f.timestamp),
        details: json!({"type":if firm.is_some(){"wallet_simulation"}else{"indicative_price"},
            "market":p.market.address,"builder":builder.name(),"target_block":f.target,"slot":f.slot,"trader":payer,
            "owner_minimum_raw":firm.map(|v|v.1.to_string()),"gas_used":firm.map(|_|one.gas_used.to::<u64>())}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;

    #[test]
    fn two_builders_keep_only_the_registered_book_and_expire_independently() {
        let rows = super::super::tests::recommendation("bopamm.roles");
        let p = Profile::parse(&json!(rows.split(',').collect::<Vec<_>>())).unwrap();
        let data = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/bopamm_ethereum_frames.json.gz"
        );
        let archive: Vec<Value> = serde_json::from_reader(GzDecoder::new(data.as_slice())).unwrap();
        for (name, kind) in [
            ("Titan", Builder::Titan),
            ("BuilderNet", Builder::BuilderNet),
        ] {
            let sample = archive
                .iter()
                .map(|x| &x[name])
                .find(|v| v.get("stateDiff").is_some())
                .unwrap();
            let registry = format!("{:#x}", p.registry.address);
            let row = json!({"maker":"Bebop","quoteAddress":sample["key"],"blockNumber":sample["target"],
                "slot":sample["slot"],"stateOverride":{(registry.clone()):{"stateDiff":sample["stateDiff"],"nonce":"0x77"}}});
            let input = |v: Value| match kind {
                Builder::Titan => json!({"venues":[v]}),
                Builder::BuilderNet => {
                    json!({"blockNumber":sample["target"],"slot":sample["slot"],(sample["key"].as_str().unwrap()):v})
                }
            };
            let parsed = frame(kind, &input(row.clone()), &p).unwrap();
            assert_eq!(
                parsed.overrides(),
                json!({(registry):{"stateDiff":sample["stateDiff"]}})
            );
            assert!(parsed.current(
                parsed.target - 1,
                parsed.timestamp - 12,
                parsed.timestamp - 1
            ));
            assert!(!parsed.current(parsed.target - 1, parsed.timestamp - 12, parsed.timestamp));
            let mut bad = row.clone();
            bad["stateOverride"]["0x0000000000000000000000000000000000000001"] =
                json!({"stateDiff":sample["stateDiff"]});
            assert!(frame(kind, &input(bad), &p).is_none());
            let duplicate = match kind {
                Builder::Titan => json!({"venues":[row,row]}),
                Builder::BuilderNet => json!({"blockNumber":sample["target"],"slot":sample["slot"],
                (format!("{:#x}",p.market.address)):row,(format!("{:#x}",p.pricing.address)):row}),
            };
            assert!(frame(kind, &duplicate, &p).is_none());
        }
    }
}
