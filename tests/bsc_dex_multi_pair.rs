//! Файл: tests/bsc_dex_multi_pair.rs
//! Что делает: решение 295, тест (а) — пул с 3 и более монетами при dex.multi_pair = да даёт в справочнике и во «взятых»
//! ВСЕ нужные пары (каждая — своя `PoolRef::Multi`), без настройки — одна запись `Contract`, как до решения. Проход —
//! настоящий index::take_with (режим, ключ строки с парой, развёртка недостающих пар по монетам пула, вторая проверка);
//! проверка кандидата — те же разборы, что на узле (verify_curve::decide, verify_balancer::decide,
//! verify_curve_ng::decide; Saddle — verify_woo::found, как verify_stable_u.rs) на ЖИВЫХ записанных ответах, плюс
//! multi_pair::adopt. Индексатор даёт одну пару пула (строка справочника: первая монета пула — встречный).
//! Данные: ellipsis_/nerve_2026-11-15 (3pool), balancer_acsi_2026-11-15 (4 и 5 токенов), curve_ng_2026-11-14 (3 монеты).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair.
//! От чего зависит: tests/multi_common (записи, окружение), arb::venues::bsc_dex::{pools, directory}.

mod multi_common;

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::gateway::errors::GatewayResult;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::directory::TokenEntry;
use arb::venues::bsc_dex::directory::gt_parse::IndexedPool;
use arb::venues::bsc_dex::pools::verify::{Candidate, Verdict};
use arb::venues::bsc_dex::pools::{
    index_pass, load_curve, load_saddle, multi_pair, verify_balancer, verify_curve, verify_curve_ng, verify_woo,
};

use multi_common::{answer, ctx, rec, set_dex};

/// Адрес из строки JSON (или слова ответа — последние 20 байт). Паника — не адрес.
fn addr(v: &Value) -> Address {
    let s = v.as_str().unwrap();
    if s.len() == 66 {
        return Address::from_slice(&alloy_primitives::hex::decode(&s[2..]).unwrap()[12..32]);
    }
    s.parse().unwrap()
}

/// Ответы проверки Ellipsis: coins(0..3) записи (coins(3) откатился — конец списка монет).
fn ellipsis_answers() -> Vec<Result<Bytes, String>> {
    let p = rec("ellipsis_2026-11-15", "probe.json");
    (0..4).map(|k| answer(&p, &format!("ellipsis coins({k})"))).collect()
}

/// Ответы второго прохода проверки Balancer по записи пула (как tests/bsc_dex_balancer_acsi_live.rs second).
fn balancer_answers(p: &Value) -> Vec<Result<Bytes, String>> {
    let dec = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
    let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
    let bal: Vec<U256> = p["balances"].as_array().unwrap().iter().map(dec).collect();
    let ret = |b: bool| Ok(Bytes::from(b.abi_encode()));
    vec![
        Ok(Bytes::from((addr(&p["getPool"][0]), dec(&p["getPool"][1])).abi_encode_params())),
        Ok(Bytes::from((tokens, bal, dec(&p["lastChangeBlock"])).abi_encode_params())),
        ret(p["isPoolFromFactory"]["weighted"] == "1"),
        ret(p["isPoolFromFactory"]["stable"] == "1"),
    ]
}

/// Ответы проверки Curve NG по записи пула: get_coins, is_meta (фабрика), version (пул).
fn ng_answers(p: &Value) -> Vec<Result<Bytes, String>> {
    let h = |s: &Value| Ok(Bytes::from(alloy_primitives::hex::decode(&s.as_str().unwrap()[2..]).unwrap()));
    vec![h(&p["factory"]["get_coins"]), h(&p["factory"]["is_meta"]), h(&p["calls"]["version"])]
}

/// Решение по кандидату — разбор проверки его вида на записанных ответах + multi_pair::adopt. Ошибок нет.
fn decide(ctx: &Ctx, c: &Candidate) -> Verdict {
    let pool = Address::from_str(&c.pool).unwrap();
    let r = match c.family.id {
        "ellipsis" => {
            let a = ellipsis_answers();
            verify_curve::decide(c, pool, &a).map(|f| (f, load_curve::coins_of(&a).unwrap()))
        }
        "nerve" => {
            let p = rec("nerve_2026-11-15", "probe.json");
            let a: Vec<_> = (0..4).map(|k| answer(&p, &format!("nerve getToken({k})"))).collect();
            let t = load_saddle::tokens_of(&a).unwrap();
            let ok = t.contains(&c.coin) && t.contains(&c.quote);
            ok.then(|| (verify_woo::found(c, PoolRef::Contract(pool)), t)).ok_or("нет пары".to_string())
        }
        "acsi-finance" => {
            let probe = rec("balancer_acsi_2026-11-15", "probe.json");
            let p = probe["pools"].as_array().unwrap().iter().find(|p| p["pool"] == c.pool.as_str()).unwrap();
            let plan = (pool, addr(&probe["vault"]["address"]), addr(&probe["factories"]["weighted"]["address"]), Some(addr(&probe["factories"]["stable"]["address"])));
            let t: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
            verify_balancer::decide(c, plan, &balancer_answers(p)).map(|f| (f, t))
        }
        _ => {
            let probe = rec("curve_ng_2026-11-14", "probe.json");
            let p = probe["pools"].as_array().unwrap().iter().find(|p| p["pool"] == c.pool.as_str()).unwrap();
            let t: Vec<Address> = p["coins"].as_array().unwrap().iter().map(addr).collect();
            verify_curve_ng::decide(c, pool, &["v7.0.0".to_string()], &ng_answers(p)).map(|f| (f, t))
        }
    };
    match r {
        Ok((f, t)) => Verdict::Taken(Box::new(multi_pair::adopt(ctx, f, &t))),
        Err(w) => Verdict::Rejected(w),
    }
}

/// Проверка пачки для take_with (без сети).
fn verifier<'a>(ctx: &'a Ctx, cands: &'a [Candidate]) -> Pin<Box<dyn Future<Output = GatewayResult<Vec<Verdict>>> + Send + 'a>> {
    Box::pin(async move { Ok(cands.iter().map(|c| decide(ctx, c)).collect()) })
}

/// Справочник и проход. Принимает: id DEX, вид, пул, монеты, встречные, режим. Возвращает: ссылки взятых.
async fn run(id: &str, kind: &str, pool: &str, tokens: &[Address], quotes: &[Address], on: bool) -> Vec<PoolRef> {
    let c = ctx(&format!("{id}-{on}-{}", quotes.len())).await;
    set_dex(&c, id, "dex.kind", json!(kind)).await;
    set_dex(&c, id, "dex.pool_enabled", json!(true)).await;
    // Настройки, без которых DEX не подключена (families::required), — значения из записей.
    match kind {
        "balancer_v2" => {
            let probe = rec("balancer_acsi_2026-11-15", "probe.json");
            set_dex(&c, id, "dex.vault", probe["vault"]["address"].clone()).await;
            set_dex(&c, id, "dex.factory", probe["factories"]["weighted"]["address"].clone()).await;
        }
        "curve_stableswap_ng" => {
            set_dex(&c, id, "dex.factory", rec("curve_ng_2026-11-14", "probe.json")["factory"].clone()).await;
            set_dex(&c, id, "dex.curve_ng_versions", json!(["v7.0.0"])).await;
        }
        _ => set_dex(&c, id, "dex.stable_pools", json!([pool])).await,
    }
    if on {
        set_dex(&c, id, "dex.multi_pair", json!(true)).await;
    }
    let coins: Vec<Address> = tokens.iter().copied().filter(|t| *t != quotes[0]).collect();
    c.index.with(|s| {
        s.quotes = quotes.iter().copied().collect();
        for (i, coin) in coins.iter().enumerate() {
            // Индексатор даёт одну пару пула — строка только у первой монеты.
            let pools = if i == 0 {
                vec![IndexedPool { dex_id: id.into(), pool: pool.into(), base: *coin, quote: quotes[0], base_symbol: String::new(), quote_symbol: String::new(), reserve_usd: None }]
            } else {
                Vec::new()
            };
            s.tokens.insert(*coin, TokenEntry { ticker: format!("{coin:#x}"), pools, ..Default::default() });
        }
    });
    let took = index_pass::take_with(&c, None, verifier).await.unwrap();
    let mut refs: Vec<PoolRef> = took.found.iter().map(|f| f.reference.clone()).collect();
    refs.sort_by_key(|r| format!("{r:?}"));
    refs
}

/// Проверка итога: в режиме — n разных Multi одного контракта; без — одна Contract. Ошибок нет (паника теста).
fn assert_pairs(on: &[PoolRef], off: &[PoolRef], pool: Address, n: usize) {
    assert_eq!(on.len(), n, "{on:?}");
    assert!(on.iter().all(|r| matches!(r, PoolRef::Multi { contract, .. } if *contract == pool)), "{on:?}");
    assert_eq!(on.iter().collect::<HashSet<_>>().len(), n, "пары разные");
    assert_eq!(off, &[PoolRef::Contract(pool)], "без dex.multi_pair — одна запись Contract");
}

/// Пул и монеты 3pool записи (BUSD, USDC, USDT). Ошибок нет.
fn three(dir: &str) -> (Address, Vec<Address>) {
    let (coins, probe) = (rec(dir, "coins.json"), rec(dir, "probe.json"));
    (addr(&probe["pool"]), ["BUSD", "USDC", "USDT"].iter().map(|s| multi_common::coin(&coins, s)).collect())
}

#[tokio::test]
async fn nerve_and_ellipsis_all_pairs() {
    for (dir, id, kind) in [("ellipsis_2026-11-15", "ellipsis", "curve_stableswap"), ("nerve_2026-11-15", "nerve", "saddle_stableswap")] {
        let (pool, t) = three(dir);
        let p = format!("{pool:#x}");
        // Встречный USDT: монеты подписки BUSD и USDC — две пары с USDT.
        let on = run(id, kind, &p, &t, &[t[2]], true).await;
        let off = run(id, kind, &p, &t, &[t[2]], false).await;
        assert_pairs(&on, &off, pool, 2);
        // Ещё промежуточная USDC (она же монета подписки): BUSD–USDT, BUSD–USDC, USDC–USDT — три пары.
        let on = run(id, kind, &p, &t, &[t[2], t[1]], true).await;
        assert_eq!(on.len(), 3, "{dir}: {on:?}");
    }
}

#[tokio::test]
async fn balancer_four_and_five_tokens() {
    let probe = rec("balancer_acsi_2026-11-15", "probe.json");
    for a in ["0x894ed9026de37afd9cce1e6c0be7d6b510e3ffe5", "0xe901c4e9a56a175cb5c94bfb5f821ebd5fc826c5"] {
        let p = probe["pools"].as_array().unwrap().iter().find(|p| p["pool"] == a).unwrap();
        let t: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
        let on = run("acsi-finance", "balancer_v2", a, &t, &[t[0]], true).await;
        let off = run("acsi-finance", "balancer_v2", a, &t, &[t[0]], false).await;
        assert_pairs(&on, &off, a.parse().unwrap(), t.len() - 1);
    }
}

#[tokio::test]
async fn curve_ng_three_coins() {
    let probe = rec("curve_ng_2026-11-14", "probe.json");
    let p = probe["pools"].as_array().unwrap().iter().find(|p| p["coins"].as_array().unwrap().len() == 3).unwrap();
    let a = p["pool"].as_str().unwrap();
    let t: Vec<Address> = p["coins"].as_array().unwrap().iter().map(addr).collect();
    let on = run("curve-bsc", "curve_stableswap_ng", a, &t, &[t[0]], true).await;
    let off = run("curve-bsc", "curve_stableswap_ng", a, &t, &[t[0]], false).await;
    assert_pairs(&on, &off, a.parse().unwrap(), 2);
}
