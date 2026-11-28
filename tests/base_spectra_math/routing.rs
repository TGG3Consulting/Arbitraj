//! Прогон обычного шага графа и кодировщика V5 по записям физической пары IBT/PT.
//! Пополнение IBT, обмен и погашение являются отдельными операциями.
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256};
use arb::core::types::{Pool, PoolRef, PoolState, Side, VenueId};
use arb::venues::bsc_dex::{
    ctx::{Ctx, Place}, exec::executor::{abi::ArbExecutor, entry}, num,
    pools::{families, load_crypto_v1, load_curve_ng, registry::Entry},
    quote::{pool_quote, route_calc, select::Usable},
};
use rust_decimal::Decimal;
use serde_json::Value;
use super::{curve_crypto_common, multi_common};

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

fn number(v: &Value) -> U256 { v.as_str().unwrap().parse().unwrap() }

fn state(ctx: &Ctx, r: &Value, pool: Address, coins: [Address; 2], stamp: u64) -> PoolState {
    let pair = (coins[0].min(coins[1]), coins[0].max(coins[1]));
    if r["kind"] == "legacy" {
        let got = curve_crypto_common::answers(r, &load_crypto_v1::calls_with(pool, 2, false));
        let decimals = r["decimals"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
        let raw = load_crypto_v1::Raw {pool, n: 2, got, now: stamp, decimals, killed: false};
        return load_crypto_v1::state(ctx, &raw, pair).unwrap();
    }
    let c = &r["calls"];
    let mut values: Vec<_> = ["A_precise()", "fee()", "offpeg_fee_multiplier()"].map(|key| number(&c[key]).abi_encode()).into();
    let rates: Vec<_> = c["stored_rates()"].as_array().unwrap().iter().map(number).collect();
    values.push(rates.abi_encode());
    for (i, coin) in coins.iter().enumerate() {
        values.push(coin.abi_encode());
        values.push(number(&r["balances"][i]).abi_encode());
    }
    let got = values.into_iter().map(|v| Ok(Bytes::from(v))).collect();
    load_curve_ng::state(ctx, &load_curve_ng::Raw { pool, n: 2, got }, pair).unwrap()
}

#[tokio::test]
async fn target_buy_and_forward_sale_use_registered_currencies_and_the_paid_v5_step() {
    let d = archive(include_bytes!("../data/first_run_dex_2026-11-26/spectra_base_directory.json.gz"));
    let m = archive(include_bytes!("../data/first_run_dex_2026-11-26/spectra_base_math.json.gz"));
    let w = archive(include_bytes!("../data/first_run_dex_2026-11-26/spectra_base_wallets.json.gz"));
    assert_eq!(d["selector"], m["selector"]);
    assert_eq!(d["selector"], w["selector"]);
    let profiles: Value = serde_json::from_str(include_str!("../../config/first_run_dex.json")).unwrap();
    let mut ctx = multi_common::ctx("spectra_target").await;
    ctx.place = Place { id: "base_dex", all: &["base_dex"] };
    let profiles = profiles.as_array().unwrap();
    let mut exercised = 0;
    let mut via_ibt = Vec::new();
    for pool in [
        "0x39e6af30ea89034d1bdd2d1cfba88caf8464fa65", // legacy, зрелый PT
        "0x6d30d3bb85232bee157cb110f5055738244f0edd", // тот же IBT, другой PT
        "0x41a2f6afd4eb02be3acb6e191230fb576a00a052", // NG, IBT на кошельке до обмена
        "0x093956b337ce2c30bed2df0960aead883d7106ec", // NG до maturity
    ] {
        let find = |doc: &Value| doc["markets"].as_array().unwrap().iter().position(|r| r["pool"] == pool).unwrap();
        let row = &d["markets"][find(&d)];
        let math = &m["markets"][find(&m)];
        let ws = &w["markets"][find(&w)]["wallets"];
        let wallet = ws.as_array().unwrap().iter().find(|r| r["mode"] == "v5" && r["legs"][1]["stage"] == "accepted").unwrap();
        let profile = profiles.iter().find(|p| p["settings"]["dex.factory"] == row["factory"]).unwrap();
        let f = families::make(profile["id"].as_str().unwrap(), profile["settings"]["dex.kind"].as_str().unwrap()).unwrap();
        let coins: [Address; 2] = ["coin0","coin1"].map(|k| row[k].as_str().unwrap().parse().unwrap());
        let dec = ["decimals0","decimals1"].map(|k| row[k].as_u64().unwrap() as u8);
        let addr = pool.parse().unwrap();
        let block = d["block"].as_u64().unwrap();
        let stamp = d["timestamp"].as_u64().unwrap();
        let pool_data = Pool { venue: VenueId("base_dex".into()), family: f.id.into(), kind: f.kind,
            reference: PoolRef::Contract(addr), token0: coins[0].min(coins[1]), token1: coins[0].max(coins[1]),
            fee_pips: None, tick_spacing: None, bin_step: None, hooks: None, state: state(&ctx, math, addr, coins, stamp),
            block, received_at: chrono::DateTime::from_timestamp(stamp as i64, 0).unwrap() };
        let e = Entry { family: f, pool: pool_data, ticker: "PT".into(), coin: coins[1], usdt: coins[0],
            key: None, ticks: None, bins: None, synced_block: block, synced_log: None, valid: true, asleep: false, state_gen: 0 };
        let underlying = row["underlying"].as_str().unwrap().parse().unwrap();
        for exact_in in [false, true] {
            assert!(pool_quote::swap(&ctx,&e,underlying,exact_in,U256::ONE).is_err());
        }
        for (i, leg) in wallet["legs"].as_array().unwrap().iter().enumerate() {
            let j = 1 - i;
            let calls = leg["calls"].as_array().unwrap();
            let data: Bytes = calls[calls.len()-5]["data"].as_str().unwrap().parse().unwrap();
            let real = ArbExecutor::executeCall::abi_decode(&data).unwrap();
            assert_eq!((real.tokenIn, real.tokenOut, real.amountIn, real.minOut), (coins[i], coins[j], number(&leg["paid"]), number(&leg["net"])));
            let [step] = real.steps.as_slice() else { panic!("ожидался один шаг V5") };
            let (params, to) = entry::params(&ctx, &e, coins[i]).unwrap();
            assert_eq!((to,params.kind(),params.data()), (step.pool,step.kind,step.data.clone()));
            assert_eq!((step.tokenIn,step.tokenOut), (coins[i],coins[j]));
            let q = math["quotes"].as_array().unwrap().iter().find(|q| q["i"].as_u64() == Some(i as u64) && q["dy"].is_string() && !number(&q["dy"]).is_zero()).unwrap();
            let dx = number(&q["dx"]);
            let want = number(&q["dy"]);
            let hop = route_calc::Leg { entry:e.clone(), usable:Usable { one_share_only:false,
                gas:leg["gas"].as_u64().unwrap(), hook_pct:Decimal::ZERO,tax_pct:Decimal::ZERO,hook:Default::default() },
                token_in:coins[i],token_out:coins[j],dec_in:dec[i],dec_out:dec[j] };
            let legs = std::slice::from_ref(&hop);
            let sale = route_calc::eval(&ctx,legs,Side::Sell,num::to_decimal(dx,dec[i]).unwrap(),Decimal::ONE).unwrap();
            assert_eq!(sale.hops[0].swapped.amount_out,want);
            let buy = route_calc::eval(&ctx,legs,Side::Buy,num::to_decimal(want,dec[j]).unwrap(),Decimal::ONE).unwrap();
            let fill = &buy.hops[0].swapped;
            assert!(fill.amount_in <= dx && fill.amount_out >= want);
            assert_eq!(pool_quote::swap(&ctx,&e,coins[i],true,fill.amount_in).unwrap().amount_out,fill.amount_out);
            assert!(pool_quote::swap(&ctx,&e,coins[i],true,fill.amount_in-U256::ONE).ok().is_none_or(|v|v.amount_out<want));
            if (pool == "0x39e6af30ea89034d1bdd2d1cfba88caf8464fa65" && i == 1)
                || (pool == "0x6d30d3bb85232bee157cb110f5055738244f0edd" && i == 0) {
                via_ibt.push(hop);
            }
            exercised += 1;
        }
    }
    assert_eq!(exercised, 8);
    // Два независимо подтверждённых пула. Эта проверка оценивает путь PT→IBT→PT в графе.
    let [first, second] = via_ibt.as_slice() else { panic!("нет двух рынков общего IBT") };
    assert_eq!(first.token_out, second.token_in);
    assert_ne!(first.token_in, second.token_out);
    let second_market = w["markets"].as_array().unwrap().iter().find(|r| r["pool"] == "0x6d30d3bb85232bee157cb110f5055738244f0edd").unwrap();
    let sampled = second_market["wallets"].as_array().unwrap().iter()
        .find(|r| r["mode"] == "v5" && r["legs"][0]["stage"] == "accepted").unwrap();
    let target = number(&sampled["legs"][0]["net"]);
    let path = route_calc::eval(&ctx, &via_ibt, Side::Buy, num::to_decimal(target, second.dec_out).unwrap(), Decimal::ONE).unwrap();
    let left = &path.hops[0].swapped;
    let right = &path.hops[1].swapped;
    assert!(left.amount_out >= right.amount_in);
    let terminal = pool_quote::swap(&ctx, &second.entry, second.token_in, true, left.amount_out).unwrap();
    assert!(terminal.amount_out >= target);
}
