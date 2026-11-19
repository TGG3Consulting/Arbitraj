//! Файл: tests/bsc_dex_probe_shallow_t178.rs
//! Что делает: решение 393 (задача t178) — мелкий или перекошенный пул больше не закрывает монету целиком.
//! На ЖИВОЙ записи 2026-10-25 (tests/data/depth_2026-10-25/responses.json, у каждого пула свой блок):
//! - пары V2 0x594ecc0d… (0,108 монеты / 131 встречного), 0x28316d69… (0,79 / 4,29), 0xc31881aa… (0,0000666 / 0,456 —
//!   монета в пуле стоит ≈ 6850 встречного: «палка»), 0xa135061c… (109 / 0,000172): по порогу глубины (10 в полосе ±2 %)
//!   все мелкие в оба направления (до решения 393 — монета «не проверено», все её направления закрыты), а пробный круг
//!   на check.probe_usdt = 20 проходит целиком — все четыре идут в пробу налогов с полной суммой; монета ждёт не
//!   «глубокого», а «годного для пробы» пула (tax_deep::has_deep); при источнике ws такие пулы не спят (pools/depth.rs);
//! - пул AAVE Uniswap V3 0x8e3ecc0b… при пороге 2000 (глубина ≈ 800 в ±2 %) — мелкий, но пробный круг (продажа
//!   остатка — на состоянии пула после покупки, quote/pool_after.rs) проходит;
//! - пул OPN Infinity CL 0x3ce99c11… — ликвидность на текущем тике 0, позиции ниже цены (тик 21200 при текущем 24499):
//!   пул НЕ пустой (quote/emptiness.rs), покупка проходит «дыру» (сторона Buy проходит), продажа — нет (выше цены
//!   ликвидности нет); пробный круг у вида CL считается на текущем состоянии — не проходит (честно, без пробы);
//!   пулы ZKP Uniswap V4 с ликвидностью 0 и без тиков в карте — по-прежнему пустые.
//!
//! ЧИСТАЯ ЛОГИКА (честно): наименьший выход min_units_for, ряд сумм halve, порог крошечного выхода пробы
//! (check/probe_tiny.rs) — на числах теста; записи eth_simulateV1 пробы на мелком пуле нет (docs/TASKS.md).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, quote::{side_fit, select,
//! emptiness}, check::{taxes_util, tax_deep, probe_tiny}, pools::depth}.

mod depth_common;
mod native_common;

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, Side};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::probe::SellMiss;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::check::{probe_tiny, tax_deep, taxes_util::probe_pools};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::depth;
use arb::venues::bsc_dex::quote::probe_quote::Way;
use arb::venues::bsc_dex::quote::{emptiness, pool_after, select, shallow_pick, side_fit};

use depth_common::{aave_v3, cl_entry, rec, v2};

/// Пары V2 записи: монета — токен0, встречный — токен1 (у каждой пары это один и тот же условный токен записи).
const PAIRS: [&str; 4] = [
    "0x594eccd06f64536bb278af508bf786c99574538a",
    "0x28316d69db3547428dee6a0e2053754dba9d42ab",
    "0xc31881aa86a2b3ca2ee8e5c8979c5fbcc376426e",
    "0xa135061c34abdeb1cd6ec492d8355318b5c8afe2",
];

/// Настройки зоны bsc_dex для теста: полоса 2 %, гистерезис 10 %, порог `min_usd`, проба 20, допуск 0,11 %, предел
/// мелких пулов на пару `per_pair`.
async fn settings(ctx: &Ctx, min_usd: u32, per_pair: u32) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.min_pool_liquidity_usd", json!(min_usd)),
        ("check.probe_usdt", json!(20)),
        ("dex.quoter_max_diff_pct", json!(0.11)),
        ("dex.shallow_pools_per_pair", json!(per_pair)),
    ] {
        ctx.settings
            .set(n, v.clone(), Some(x), "тест")
            .await
            .unwrap();
    }
}

/// Встречный токен — «USDT» для токенов бота (18 знаков). Ошибок нет.
fn usdt_is(p: &Parts, usdt: Address) {
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(usdt),
    }]);
    p.tokens.remember_decimals(usdt, 18);
}

#[tokio::test]
async fn shallow_v2_pairs_go_to_probe_with_full_sum() {
    let ctx = native_common::ctx("t178-v2").await;
    settings(&ctx, 10, 4).await;
    let p = Parts::new(ctx.clone());
    let r = rec();
    let pairs: Vec<_> = PAIRS.iter().map(|a| v2(&r, a, true)).collect();
    usdt_is(&p, pairs[0].usdt);
    for e in &pairs {
        p.registry.put(e.clone());
    }
    let cap = U256::from(20u64) * U256::from(10u64).pow(U256::from(18u8));
    let min = side_fit::min_units(&ctx).unwrap();
    assert_eq!(min, U256::from(910u32), "⌈100 / 0,11⌉");
    for e in &pairs {
        // До решения 393: мелкий в оба направления — в пробу не шёл.
        let why = select::deep_side(&ctx, &p.registry, &p.tokens, e, 18, select::Need::Both)
            .unwrap()
            .expect("по порогу глубины пул мелкий");
        eprintln!("{:?}: {why}", e.pool.reference);
        let t = side_fit::round_trip(&ctx, e, cap, min).expect("пробный круг проходит");
        assert_eq!(t.amount, cap, "V2 берёт всю сумму пробы");
        assert!(t.coins >= min);
        for s in [Side::Buy, Side::Sell] {
            assert!(
                side_fit::side_usd(&ctx, &p.registry, &p.tokens, 18, e, s).is_some(),
                "сторона {s:?} проходит"
            );
        }
    }
    let (pools, env, shallow) =
        probe_pools(&ctx, &p.registry, &p.tokens, &pairs[0].coin, 18).unwrap();
    assert!(env.is_empty() && shallow.is_empty(), "{env:?} {shallow:?}");
    assert_eq!(pools.len(), PAIRS.len());
    assert!(pools.iter().all(|(_, a)| *a == Some(cap)));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pp = p.pools(tx);
    assert!(
        tax_deep::has_deep(&pp, &pairs[0].coin),
        "монета не ждёт «глубокого» пула"
    );
    // При ws мелкий пул, который пропускает сумму пробы, слушается целиком (решение 393, п. 5 разбора).
    for e in &pairs {
        assert!(
            depth::deep(&pp, &e.pool.reference).await,
            "{:?} не спит",
            e.pool.reference
        );
    }
}

#[tokio::test]
async fn without_probe_settings_shallow_stays_as_before() {
    let ctx = native_common::ctx("t178-old").await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.min_pool_liquidity_usd", json!(10)),
    ] {
        ctx.settings
            .set(n, v.clone(), Some(x), "тест")
            .await
            .unwrap();
    }
    let p = Parts::new(ctx.clone());
    let e = v2(&rec(), PAIRS[2], true);
    usdt_is(&p, e.usdt);
    p.registry.put(e.clone());
    // dex.shallow_pools_per_pair не задана — мелкие пулы не допускаются, пул — мелкий с причиной (как до решения 393).
    let (pools, env, shallow) = probe_pools(&ctx, &p.registry, &p.tokens, &e.coin, 18).unwrap();
    assert!(pools.is_empty() && env.is_empty());
    assert_eq!(shallow.len(), 1);
    assert!(
        shallow[0].contains("dex.shallow_pools_per_pair"),
        "{}",
        shallow[0]
    );
}

/// Ревью t178 (Б2, повторное — Н-B1): предел 1 на пару. Все четыре мелкие пары записи пропускают полную сумму пробы
/// (уровень 0) — при равном уровне решает адрес: 0x28316d69… (глубина в ранг не входит). Остальные в пробу не идут и при
/// ws спят. Серия обменов туда-обратно (глубины пар меняются, у 0xc31881aa… после покупки она больше всех) состав
/// отбора не меняет (отбор устойчив к обменам).
#[tokio::test]
async fn shallow_limit_picks_by_rank() {
    let ctx = native_common::ctx("t178-limit").await;
    settings(&ctx, 10, 1).await;
    let p = Parts::new(ctx.clone());
    let r = rec();
    let mut pairs: Vec<_> = PAIRS.iter().map(|a| v2(&r, a, true)).collect();
    usdt_is(&p, pairs[0].usdt);
    for e in &pairs {
        p.registry.put(e.clone());
    }
    let chosen = pairs[1].pool.reference.clone();
    let (pools, _, shallow) =
        probe_pools(&ctx, &p.registry, &p.tokens, &pairs[0].coin, 18).unwrap();
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].0.pool.reference, chosen);
    assert_eq!(shallow.len(), 3);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pp = p.pools(tx);
    assert!(depth::deep(&pp, &chosen).await);
    for e in pairs.iter().filter(|e| e.pool.reference != chosen) {
        assert!(
            !depth::deep(&pp, &e.pool.reference).await,
            "{:?} спит",
            e.pool.reference
        );
    }
    // Обмены туда-обратно в каждой паре: купить на 300 встречного, продать купленное назад (расчёт пары V2 и состояние
    // после обмена — quote/pool_after.rs, как исполнит пара). После каждого шага — запись в реестр и тот же отбор.
    let pair = (pairs[0].coin, pairs[0].usdt);
    let big = U256::from(300u64) * U256::from(10u64).pow(U256::from(18u8));
    for _round in 0..3 {
        for (k, pr) in pairs.iter_mut().enumerate() {
            let e = pr.clone();
            let (bought, after) = pool_after::apply(&ctx, &e, e.usdt, big).unwrap();
            pr.pool.state = after;
            p.registry.put(pr.clone());
            let got = shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair);
            assert_eq!(*got, vec![chosen.clone()], "после покупки в паре {k}");
            let e = pr.clone();
            let (_, back) = pool_after::apply(&ctx, &e, e.coin, bought.amount_out).unwrap();
            pr.pool.state = back;
            p.registry.put(pr.clone());
            let got = shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair);
            assert_eq!(*got, vec![chosen.clone()], "после продажи в паре {k}");
        }
    }
    // Предел 0 — как до решения 393.
    ctx.settings
        .set(
            "dex.shallow_pools_per_pair",
            Scope::Venue("bsc_dex".into()),
            Some(json!(0)),
            "тест",
        )
        .await
        .unwrap();
    assert!(!depth::deep(&pp, &chosen).await);
}

/// ЧИСТАЯ ЛОГИКА (повторное ревью t178, Н-B1): порядок отбора — уровень, при равном уровне уже отобранный остаётся,
/// затем адрес; претендент со строго лучшим уровнем вытесняет.
#[test]
fn pick_order_keeps_incumbent_pure_logic() {
    use arb::core::types::PoolRef;
    let a = PoolRef::Contract(Address::from([0xaa; 20]));
    let b = PoolRef::Contract(Address::from([0xbb; 20]));
    let cand = |lv: u32, was: bool, r: &PoolRef| (lv, was, format!("{r:?}"), r.clone());
    // Равный уровень: без прошлого отбора — меньший адрес; с прошлым — прежний (b), хотя адрес больше.
    assert_eq!(
        shallow_pick::order(vec![cand(0, false, &b), cand(0, false, &a)], 1),
        vec![a.clone()]
    );
    assert_eq!(
        shallow_pick::order(vec![cand(0, true, &b), cand(0, false, &a)], 1),
        vec![b.clone()]
    );
    // Претендент лучше уровнем (больше проходящая доля пробы) — вытесняет прежнего.
    assert_eq!(
        shallow_pick::order(vec![cand(1, true, &b), cand(0, false, &a)], 1),
        vec![a.clone()]
    );
    assert_eq!(
        shallow_pick::order(vec![cand(0, true, &b), cand(0, false, &a)], 2),
        vec![b, a]
    );
}

/// ЧИСТАЯ ЛОГИКА (ревью t178, Б1): прежний итог при «не проверено». Ловушка, включённая у известной монеты (продажа
/// откатилась в глубоком пуле — вердикт по пулу), не прячется за помехой на мелком: Doubt снимает «прошла».
#[test]
fn doubt_drops_known_pure_logic() {
    use arb::core::types::{CheckState, TokenTaxes};
    use arb::venues::bsc_dex::check::pool_taxes::outcome;
    use arb::venues::bsc_dex::check::state::PoolTaxes;
    use arb::venues::bsc_dex::check::taxes::Outcome;
    use arb::venues::bsc_dex::check::taxes_util::keeps;
    let known = Some(CheckState::Known(TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }));
    let failed = Some(CheckState::Failed {
        reason: "ловушка".into(),
    });
    // Проба: покупка в глубоком пуле прошла, продажа откатилась (вердикт по пулу); у мелкого — помеха узла.
    let bought = [PoolTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: None,
        transfer_pct: Decimal::ZERO,
    }];
    let reverted = [(true, "V2: пробная продажа откатилась".to_string())];
    let env = (
        Vec::new(),
        vec!["V4 мелкий: узел 429".to_string()],
        Vec::new(),
    );
    let out = outcome(&bought, false, &reverted, env, &[]);
    assert!(matches!(out, Outcome::Doubt(_)), "{out:?}");
    assert!(!keeps(&known, true), "«прошла» снимается");
    assert!(keeps(&failed, true), "«не прошла» остаётся");
    // Только помеха (вердиктов нет) — прежний итог держится, как раньше.
    let no_quote = [(false, "V2: квотер продажи не ответил".to_string())];
    let env = (
        Vec::new(),
        vec!["V4 мелкий: узел 429".to_string()],
        Vec::new(),
    );
    assert!(matches!(
        outcome(&bought, false, &no_quote, env, &[]),
        Outcome::Wait(_)
    ));
    assert!(keeps(&known, false));
    // Покупка откатилась в одном пуле, помеха в другом — тоже Doubt.
    let env = (
        vec!["V2: покупка откатилась".to_string()],
        vec!["узел".to_string()],
        Vec::new(),
    );
    assert!(matches!(
        outcome(&[], false, &[], env, &[]),
        Outcome::Doubt(_)
    ));
}

#[tokio::test]
async fn aave_v3_below_threshold_still_probed() {
    let ctx = native_common::ctx("t178-aave").await;
    settings(&ctx, 2000, 1).await;
    let p = Parts::new(ctx.clone());
    let (e, _, _) = aave_v3(&rec());
    usdt_is(&p, e.usdt);
    p.registry.put(e.clone());
    assert!(
        select::deep_side(&ctx, &p.registry, &p.tokens, &e, 18, select::Need::Both)
            .unwrap()
            .is_some(),
        "≈ 800 USDT в ±2 % при пороге 2000 — мелкий"
    );
    let (pools, _, _) = probe_pools(&ctx, &p.registry, &p.tokens, &e.coin, 18).unwrap();
    assert_eq!(pools.len(), 1);
    let cap = U256::from(20u64) * U256::from(10u64).pow(U256::from(18u8));
    assert_eq!(pools[0].1, Some(cap));
}

#[tokio::test]
async fn cl_gap_pool_not_empty_buy_side_only() {
    let ctx = native_common::ctx("t178-gap").await;
    let r = rec();
    let pick = |id: &str| {
        let p = r
            .pools
            .iter()
            .find(|p| p["id"].as_str().is_some_and(|x| x.starts_with(id)))
            .unwrap();
        cl_entry(&r, p).unwrap()
    };
    let gap = pick("0x3ce99c11");
    assert!(matches!(
        gap.pool.state,
        arb::core::types::PoolState::Cl { liquidity: 0, .. }
    ));
    assert!(
        !emptiness::empty(&gap),
        "ликвидность рядом по тикам — пул не пуст"
    );
    let cap = U256::from(20u64) * U256::from(10u64).pow(U256::from(18u8));
    let min = U256::from(910u32);
    assert_eq!(
        side_fit::side(&ctx, &gap, Side::Buy, cap, min),
        Some(cap),
        "покупка проходит дыру"
    );
    assert_eq!(
        side_fit::side(&ctx, &gap, Side::Sell, cap, min),
        None,
        "выше цены ликвидности нет"
    );
    assert_eq!(
        side_fit::round_trip(&ctx, &gap, cap, min),
        None,
        "CL: круг на текущем состоянии не проходит"
    );
    // ZKP Uniswap V4: ликвидность 0, в карте тиков нет — пуст, как раньше.
    let empty = pick("0xbe42d277");
    assert!(emptiness::empty(&empty));
    assert_eq!(side_fit::side(&ctx, &empty, Side::Buy, cap, min), None);
}

#[test]
fn min_units_and_halving_pure_logic() {
    assert_eq!(
        side_fit::min_units_for(Decimal::from_str("0.11").unwrap()),
        Some(U256::from(910u32))
    );
    assert_eq!(
        side_fit::min_units_for(Decimal::ONE),
        Some(U256::from(100u32))
    );
    assert_eq!(side_fit::min_units_for(Decimal::ZERO), None);
    // Проходит не больше 300: ряд 1000 → взял 300 → 300 проходит.
    let got = side_fit::halve(U256::from(1000u32), |a| {
        if a <= U256::from(300u32) {
            Ok(a)
        } else {
            Err(Some(U256::from(300u32)))
        }
    });
    assert_eq!(got, Some(U256::from(300u32)));
    // Выход мал уже на полной сумме — меньшие не пробуются.
    let mut tries = 0;
    let none: Option<U256> = side_fit::halve(U256::from(1000u32), |_| {
        tries += 1;
        Err(None)
    });
    assert_eq!((none, tries), (None, 1));
}

fn ok_call(data: &[u8]) -> CallResult {
    serde_json::from_value(json!({
        "status": "0x1",
        "returnData": Bytes::copy_from_slice(data),
        "gasUsed": "0x0",
    }))
    .unwrap()
}

#[tokio::test]
async fn tiny_probe_output_is_interference_pure_logic() {
    let ctx = native_common::ctx("t178-tiny").await;
    let e = v2(&rec(), PAIRS[0], true);
    let way = Way::of(&ctx, &e);
    let min = U256::from(910u32);
    assert!(probe_tiny::buy(&ctx, &e, U256::from(909u32), min).is_some());
    assert!(probe_tiny::buy(&ctx, &e, min, min).is_none());
    let q = ok_call(&[]);
    // Продажа: ответ квотера 500 < 910 — налог не измерен, помеха (не вердикт).
    let small = probe_tiny::sell(
        &ctx,
        &e,
        way,
        &q,
        U256::from(1u8),
        min,
        Ok((U256::from(500u32), U256::ZERO)),
    );
    assert!(
        matches!(
            small,
            Err(SellMiss {
                reverted: false,
                ..
            })
        ),
        "{small:?}"
    );
    // Налог 99 % на обычном выходе — замер остаётся (ловушка ловится налогом).
    let trap = Ok((U256::from(1_000_000u32), U256::from(10_000u32)));
    assert_eq!(
        probe_tiny::sell(&ctx, &e, way, &q, U256::from(1u8), min, trap.clone()),
        trap
    );
    // Откат продажи без разборчивого ответа квотера — вердикт как раньше.
    let reverted = Err(SellMiss {
        reverted: true,
        why: "откат".into(),
    });
    assert_eq!(
        probe_tiny::sell(&ctx, &e, way, &q, U256::from(1u8), min, reverted.clone()),
        reverted
    );
}
