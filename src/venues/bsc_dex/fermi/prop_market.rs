//! Цена и выплата LambdaClass в следующем слоте конкретного строителя.
use super::{
    Builder, Ctx, Deployment, Frame, Head, Preview, Request, builder_snapshot, canonical,
    decimalsCall, rpc, slot,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    exec::prop_abi::IProp,
    pools::{families, prop_venue},
};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use chrono::Utc;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Oracle {
    pub(super) market: Address,
    pub(super) address: Address,
    pub(super) hash: B256,
    slots: BTreeSet<B256>,
}

impl Oracle {
    pub(super) fn parse(market: Address, rows: &[String]) -> Option<Self> {
        let [row] = rows else { return None };
        let mut fields = row.split_whitespace();
        let address: Address = fields.next()?.parse().ok()?;
        let hash: B256 = fields.next()?.parse().ok()?;
        let mut slots = BTreeSet::new();
        for s in fields {
            if !slots.insert(s.parse().ok()?) {
                return None;
            }
        }
        if market.is_zero()
            || address.is_zero()
            || hash.is_zero()
            || hash == keccak256([])
            || slots.is_empty()
            || slots.len() > 128
        {
            return None;
        }
        Some(Self {
            market,
            address,
            hash,
            slots,
        })
    }

    pub(super) fn frame(&self, builder: Builder, snapshot: &Value) -> Option<Frame> {
        let (row, number, slot) = match builder {
            Builder::Titan => {
                let mut rows = snapshot["venues"].as_array()?.iter().filter(|r| {
                    r["quoteAddress"]
                        .as_str()
                        .and_then(|s| s.parse::<Address>().ok())
                        == Some(self.market)
                });
                let r = rows.next()?;
                if rows.next().is_some() {
                    return None;
                }
                (r, r["blockNumber"].as_u64()?, r["slot"].as_u64()?)
            }
            Builder::BuilderNet => (
                snapshot.get(format!("{:#x}", self.market))?,
                serde_json::from_value::<U64>(snapshot["blockNumber"].clone())
                    .ok()?
                    .to(),
                snapshot["slot"].as_u64()?,
            ),
        };
        for (account, state) in row["stateOverride"].as_object()? {
            if state.get("code").is_some() || state.get("state").is_some() {
                return None;
            }
            if let Some(d) = state.get("stateDiff") {
                let updates = d.as_object()?;
                if !updates.is_empty() && account.parse::<Address>().ok() != Some(self.address) {
                    return None;
                }
                if updates.keys().any(|s| {
                    s.parse::<B256>()
                        .map_or(true, |key| !self.slots.contains(&key))
                }) {
                    return None;
                }
            }
        }
        // Frame copies only stateDiff at the accepted oracle. Account nonces and balances remain canonical.
        Frame::from_row(
            row,
            number,
            slot,
            Deployment {
                router: self.market,
                engine: self.address,
                runtime: self.hash,
            },
        )
    }
}

pub(in crate::venues::bsc_dex) async fn preview(
    ctx: &Ctx,
    id: &str,
    r: Request,
) -> GatewayResult<Preview> {
    let bad = |msg| ctx.err(ErrorCode::NotApplicable, msg);
    if ctx.chain_id() != Some(1)
        || r.buy_amount_raw.is_some()
        || r.input.is_zero()
        || r.output.is_zero()
        || r.input == r.output
    {
        return Err(bad(
            "Строитель PropAMM: нужен точный вход Ethereum и два контракта ERC20",
        ));
    }
    let rev = ctx.settings.generation();
    let key = match ctx.dex_suffix()? {
        Some(chain) => format!("{id}{chain}"),
        None => id.to_owned(),
    };
    let family =
        families::classify(ctx, &key).map_err(|_| bad("Профиль рынка отсутствует в этой сети"))?;
    let market = ctx.dex_addr("dex.factory", family.id)?;
    if r.market.is_some_and(|m| m != market) {
        return Err(bad("Выбран другой адрес рынка"));
    }
    let oracle = Oracle::parse(market, &ctx.dex_list("dex.propamm_price_slots", family.id)?)
        .ok_or_else(|| bad("Для рынка не задан проверенный адрес оракула и его ценовые слоты"))?;
    let builder = r
        .builder
        .as_deref()
        .and_then(Builder::parse)
        .ok_or_else(|| bad("Укажите titan или buildernet"))?;
    let payer = r
        .trader
        .filter(|v| !v.is_zero())
        .ok_or_else(|| bad("Укажите плательщика"))?;
    let floor = if let Some(t) = &r.firm {
        let owner = super::super::wallet::signer::address(&ctx.with_account(&t.wallet))?;
        if owner != payer || t.allow_trade || t.fee_tenths_bps.is_some() || t.maximum_raw.is_some()
        {
            return Err(bad("Условия не соответствуют проверке кошелька"));
        }
        Some(
            super::super::num::exact_amount(Some(&t.minimum_raw), None, 0)
                .filter(|v| !v.is_zero())
                .ok_or_else(|| bad("Укажите положительный минимум выплаты"))?,
        )
    } else {
        None
    };
    let (chain, h, snapshot): (U64, Head, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_chainId", json!([])),
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        builder_snapshot(ctx, builder)
    )?;
    let stale = || {
        ctx.err(
            ErrorCode::StaleData,
            "Кадр строителя, родитель блока или настройки изменились",
        )
    };
    let f = oracle.frame(builder, &snapshot).ok_or_else(stale)?;
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
    let at = json!({"blockHash":h.hash,"requireCanonical":true});
    let (state, code, precision): (_, Bytes, _) = tokio::try_join!(
        prop_venue::state(ctx, family, market, (r.input, r.output), &at),
        rpc::call(ctx, "eth_getCode", json!([oracle.address, at])),
        canonical(ctx, h.hash, r.input, decimalsCall {})
    )?;
    let (roles, enabled) = state;
    if !enabled || code.is_empty() || keccak256(code) != oracle.hash {
        return Err(bad("Код оракула или допуск рынка изменился"));
    }
    let amount = super::super::num::exact_amount(
        r.amount_raw.as_deref(),
        r.amount_tokens.as_deref(),
        precision,
    )
    .filter(|v| !v.is_zero())
    .ok_or_else(|| bad("Недопустимый размер входа"))?;
    let call = |to: Address, data: Vec<u8>| json!({"from":payer,"to":to,"data":Bytes::from(data)});
    let decoded = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Ответ не подтверждает выплату в слоте строителя",
        )
    };
    let (net, gas) = if let Some(minimum) = floor {
        let module = ctx.dex_addr("dex.router", family.id)?;
        let terms = super::prop_slot::terms(ctx, family, &roles, module, &h).await?;
        let amount_signed = alloy_primitives::I256::try_from(amount).map_err(|_| decoded())?;
        let leg =
            keccak256((h.hash, payer, market, r.input, r.output, amount, minimum).abi_encode());
        let order = super::prop_slot::Order {
            terms,
            payer,
            tokens: (r.input, r.output),
            specified: amount_signed,
            paid: amount,
            minimum,
            leg,
        }
        .proof(&h, &f, builder);
        let (_, credit, gas) = super::prop_slot::simulate(ctx, &h, &f, &order, None).await?;
        (credit, Some(gas))
    } else {
        let q = IProp::quoteVenueV1Call {
            venue: market,
            tokenIn: r.input,
            tokenOut: r.output,
            amountIn: amount,
        };
        let steps = slot::run(ctx, &h, &f, json!([call(roles.router(), q.abi_encode())])).await?;
        let [x] = steps.as_slice() else {
            return Err(decoded());
        };
        if x.status != U64::from(1) {
            return Err(decoded());
        }
        let res =
            IProp::quoteVenueV1Call::abi_decode_returns(&x.return_data).map_err(|_| decoded())?;
        if res.quotedVenue != market || res.amountOut.is_zero() {
            return Err(decoded());
        }
        (res.amountOut, None)
    };
    let (latest, snapshot): (Head, Value) = tokio::try_join!(
        rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])),
        builder_snapshot(ctx, builder)
    )?;
    let new = oracle.frame(builder, &snapshot).ok_or_else(stale)?;
    if latest.hash != h.hash
        || !current()
        || rev != ctx.settings.generation()
        || (new.target, new.slot, &new.diff) != (f.target, f.slot, &f.diff)
    {
        return Err(stale());
    }
    Ok(Preview {
        source: id.to_owned(),
        input: r.input,
        output: r.output,
        spent_raw: amount.to_string(),
        received_raw: net.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: h.hash.to_string(),
        block: Some(h.number.to()),
        expires: Some(f.timestamp),
        details: json!({"type":if floor.is_some(){"wallet_simulation"}else{"indicative_price"},"market":market,"builder":builder.name(),"target_block":f.target,
            "slot":f.slot,"trader":payer,"owner_minimum_raw":floor.map(|v|v.to_string()),"gas_used":gas}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ARCHIVE: &str = include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/prop_ethereum_builder_frames.json"
    );

    #[test]
    fn public_prices_keep_only_confirmed_storage_on_the_accepted_oracle() {
        let snapshot: Value = serde_json::from_str(ARCHIVE).unwrap();
        for source in snapshot["frames"].as_array().unwrap() {
            let market = source["market"].as_str().unwrap().parse().unwrap();
            let address: Address = source["oracle"].as_str().unwrap().parse().unwrap();
            let storage = source["storage"].as_object().unwrap();
            let keys = storage.keys().cloned().collect::<Vec<_>>().join(" ");
            let oracle = Oracle::parse(
                market,
                &[format!("{address:#x} {:#x} {keys}", B256::repeat_byte(1))],
            )
            .unwrap();
            let raw = json!({"quoteAddress":market,"blockNumber":snapshot["number"],"slot":snapshot["slot"],
                "stateOverride":{(format!("{address:#x}")):{"nonce":"0x9","balance":"0x123","stateDiff":storage}}});
            let outer = |row: &Value| {
                if source["builder"] == "Titan" {
                    json!({"venues":[row]})
                } else {
                    json!({(format!("{market:#x}")):row, "blockNumber":format!("0x{:x}",snapshot["number"].as_u64().unwrap()), "slot":snapshot["slot"]})
                }
            };
            let builder = if source["builder"] == "Titan" {
                Builder::Titan
            } else {
                Builder::BuilderNet
            };
            let frame = oracle.frame(builder, &outer(&raw)).unwrap();
            assert_eq!(
                frame.overrides(),
                json!({(format!("{address:#x}")):{"stateDiff":storage}})
            );
            for key in ["state", "code"] {
                let mut changed = raw.clone();
                changed["stateOverride"][format!("{address:#x}")][key] = json!("0xff");
                assert!(oracle.frame(builder, &outer(&changed)).is_none());
            }
            let mut foreign = raw.clone();
            foreign["stateOverride"][format!("{:#x}", Address::repeat_byte(55))] =
                json!({"stateDiff":storage});
            assert!(oracle.frame(builder, &outer(&foreign)).is_none());
            let mut unknown = raw.clone();
            unknown["stateOverride"][format!("{address:#x}")]["stateDiff"]
                [format!("{:#x}", B256::repeat_byte(55))] = json!(B256::ZERO);
            assert!(oracle.frame(builder, &outer(&unknown)).is_none());
        }
    }
}
