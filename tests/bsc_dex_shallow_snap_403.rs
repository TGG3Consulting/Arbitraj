//! Файл: tests/bsc_dex_shallow_snap_403.rs
//! Что делает: решение 403 (задача t188) — отбор мелких пулов пары считает построитель вне пути котировки и публикует
//! снимком (quote/shallow_build.rs, quote/shallow_memo.rs); путь котировки только читает снимок.
//! - На ЖИВОЙ записи 2026-10-25 (те же четыре пары V2, что tests/bsc_dex_probe_shallow_t178.rs): отбор снимка равен
//!   отбору прямого расчёта решения 393 (shallow_pick.rs calc / pick) при пределе 1, после серии обменов туда-обратно,
//!   при пределе 4 и при пределе 0 (мелкие не допускаются).
//! - Сторож по исходникам: функции пути котировки (admitted_snap, on_snap; Memo::with, Snap::admits) не берут Mutex и
//!   не читают настройки, реестр и токены; сборка графа, фоновый граф и допуск стороны к котировке (pools/graph.rs,
//!   pools/graph_eval.rs, quote/select.rs) не зовут расчёт отбора (admitted, pick, limit).
//!
//! - Ревью t188 (Н1): снимок допуска пути котировки публикуется только при смене допуска или предела — обмены без
//!   смены допуска номер публикации не меняют; смена допуска (выбранный пул стал недостоверным) — публикуется;
//!   гистерезис: вернувшийся пул с тем же рангом и меньшим адресом не вытесняет нового выбранного (прямой расчёт без
//!   прошлого отбора выбрал бы его), допуск не мигает; пара со встречным не USDT — смена пула «встречный — USDT»
//!   пересчитывает её проходом по отметкам (отпечаток курса обновлён).
//! - Сторож по исходникам (ревью t188): σ плана на сите (sigma_plan.rs entry_shared) — под замком рядов только
//!   measure_key, без настроек и format!; measure_key — без копий ключа и текста; coin_ride.rs blocks_new — атомарный
//!   быстрый путь до замка `rides`.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, quote::{shallow_build,
//! shallow_pick}, pools::registry}; исходники src/ (чтение файлов). Моков нет.

mod depth_common;
mod native_common;

use alloy_primitives::{Address, U256};
use serde_json::json;

use arb::core::types::{Asset, PoolRef};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::{pool_after, shallow_build, shallow_pick};

use depth_common::{rec, v2};

/// Пары V2 записи (как в tests/bsc_dex_probe_shallow_t178.rs): монета — токен0, встречный — один условный токен.
const PAIRS: [&str; 4] = [
    "0x594eccd06f64536bb278af508bf786c99574538a",
    "0x28316d69db3547428dee6a0e2053754dba9d42ab",
    "0xc31881aa86a2b3ca2ee8e5c8979c5fbcc376426e",
    "0xa135061c34abdeb1cd6ec492d8355318b5c8afe2",
];

/// Значение настройки зоны bsc_dex. Ошибок нет (тест падает).
async fn set(ctx: &Ctx, n: &str, x: serde_json::Value) {
    ctx.settings
        .set(n, Scope::Venue("bsc_dex".into()), Some(x), "тест")
        .await
        .unwrap();
}

/// Настройки теста t178: полоса 2 %, гистерезис 10 %, порог 10, проба 20, допуск 0,11 %, предел `per_pair`.
async fn settings(ctx: &Ctx, per_pair: u32) {
    for (n, x) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.depth_band_pct", json!(2)),
        ("dex.depth_hysteresis_pct", json!(10)),
        ("dex.min_pool_liquidity_usd", json!(10)),
        ("check.probe_usdt", json!(20)),
        ("dex.quoter_max_diff_pct", json!(0.11)),
        ("dex.shallow_pools_per_pair", json!(per_pair)),
    ] {
        set(ctx, n, x).await;
    }
}

/// Отбор пары прямым расчётом решения 393 (без снимка и без прошлого отбора). Ошибок нет.
fn direct(ctx: &Ctx, p: &Parts, pair: (Address, Address), n: usize) -> Vec<PoolRef> {
    let list = p.registry.pair_arcs(pair.0, pair.1);
    shallow_pick::calc(ctx, &p.registry, &p.tokens, 18, pair, n, &[], &list)
}

/// Допущенные по снимку пулы пары (путь котировки). Ошибок нет.
fn by_snap(p: &Parts, pairs: &[Entry]) -> Vec<PoolRef> {
    let mut v: Vec<PoolRef> = pairs
        .iter()
        .filter(|e| shallow_pick::admitted_snap(&p.registry, e))
        .map(|e| e.pool.reference.clone())
        .collect();
    v.sort_by_key(|r| format!("{r:?}"));
    v
}

/// То же, отсортированное, у списка. Ошибок нет.
fn sorted(mut v: Vec<PoolRef>) -> Vec<PoolRef> {
    v.sort_by_key(|r| format!("{r:?}"));
    v
}

#[tokio::test]
async fn snapshot_equals_direct_pick() {
    let ctx = native_common::ctx("t188-snap").await;
    settings(&ctx, 1).await;
    let p = Parts::new(ctx.clone());
    let r = rec();
    let mut pairs: Vec<_> = PAIRS.iter().map(|a| v2(&r, a, true)).collect();
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(pairs[0].usdt),
    }]);
    p.tokens.remember_decimals(pairs[0].usdt, 18);
    for e in &pairs {
        p.registry.put(e.clone());
    }
    let pair = (pairs[0].coin, pairs[0].usdt);
    let chosen = pairs[1].pool.reference.clone();
    // До прохода построителя снимок пуст: путь котировки мелкие не допускает.
    assert!(!shallow_pick::on_snap(&p.registry));
    let mut st = shallow_build::State::default();
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    assert!(shallow_pick::on_snap(&p.registry));
    assert_eq!(direct(&ctx, &p, pair, 1), vec![chosen.clone()]);
    assert_eq!(by_snap(&p, &pairs), vec![chosen.clone()]);
    assert_eq!(
        *shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair),
        vec![chosen.clone()]
    );
    // Обмены туда-обратно (как t178): после каждой записи — проход построителя, снимок = прямой расчёт = pick.
    let big = U256::from(300u64) * U256::from(10u64).pow(U256::from(18u8));
    // Ссылки и пары пулов при обменах не меняются — допуск по снимку смотрится по этой копии.
    let refs = pairs.clone();
    for (k, pr) in pairs.iter_mut().enumerate() {
        let e = pr.clone();
        let (bought, after) = pool_after::apply(&ctx, &e, e.usdt, big).unwrap();
        pr.pool.state = after;
        p.registry.put(pr.clone());
        shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
        assert_eq!(by_snap(&p, &refs), direct(&ctx, &p, pair, 1), "покупка {k}");
        assert_eq!(by_snap(&p, &refs), vec![chosen.clone()], "покупка {k}");
        let e = pr.clone();
        let (_, back) = pool_after::apply(&ctx, &e, e.coin, bought.amount_out).unwrap();
        pr.pool.state = back;
        p.registry.put(pr.clone());
        shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
        assert_eq!(
            *shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair),
            vec![chosen.clone()],
            "продажа {k}"
        );
        assert_eq!(by_snap(&p, &refs), vec![chosen.clone()], "продажа {k}");
    }
    // Предел 4 — все четыре мелкие пары (уровень 0), как прямой расчёт.
    set(&ctx, "dex.shallow_pools_per_pair", json!(4)).await;
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    let all = sorted(direct(&ctx, &p, pair, 4));
    assert_eq!(all.len(), PAIRS.len());
    assert_eq!(by_snap(&p, &pairs), all);
    // Предел 0 — мелкие не допускаются (как до решения 393).
    set(&ctx, "dex.shallow_pools_per_pair", json!(0)).await;
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    assert!(!shallow_pick::on_snap(&p.registry));
    assert!(by_snap(&p, &pairs).is_empty());
}

/// Тело функции `sig` в исходнике (от сигнатуры до парной закрывающей скобки). Ошибок нет (тест падает).
fn body<'a>(src: &'a str, sig: &str) -> &'a str {
    let at = src.find(sig).unwrap_or_else(|| panic!("нет «{sig}»"));
    let open = at + src[at..].find('{').unwrap();
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[at..open + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("не закрыта «{sig}»");
}

/// Исходник по пути от корня крейта. Ошибок нет (тест падает).
fn src(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn quote_path_reads_snapshot_only() {
    let pick = src("src/venues/bsc_dex/quote/shallow_pick.rs");
    let memo = src("src/venues/bsc_dex/quote/shallow_memo.rs");
    let bodies = [
        body(&pick, "pub fn admitted_snap("),
        body(&pick, "pub fn on_snap("),
        body(&memo, "pub fn with<R>("),
        body(&memo, "pub fn admits("),
    ];
    for b in bodies {
        for bad in [
            "ctx",
            ".lock(",
            "Mutex",
            "settings",
            ".int(",
            ".dec(",
            ".text(",
            ".usdt(",
            "by_coin",
            "by_pair",
            "pair_arcs",
            "pair_stamp",
            "tokens",
            "calc(",
            "pick(",
            "format!",
        ] {
            assert!(!b.contains(bad), "путь котировки: «{bad}» в\n{b}");
        }
    }
    for bad in ["Mutex<", "Mutex::", ".lock("] {
        assert!(!memo.contains(bad), "снимок отбора — без Mutex: «{bad}»");
    }
    for f in [
        "src/venues/bsc_dex/pools/graph.rs",
        "src/venues/bsc_dex/pools/graph_eval.rs",
        "src/venues/bsc_dex/quote/select.rs",
    ] {
        let s = src(f);
        for bad in [
            "shallow_pick::admitted(",
            "shallow_pick::pick(",
            "shallow_pick::limit(",
        ] {
            assert!(
                !s.contains(bad),
                "{f}: «{bad}» — на пути котировки только снимок"
            );
        }
    }
}

/// Пулы записи t178 в реестр, USDT = встречный токен пар; снимок после первого прохода. Ошибок нет (тест падает).
async fn setup(tag: &str, per_pair: u32) -> (Ctx, Parts, Vec<Entry>, shallow_build::State) {
    let ctx = native_common::ctx(tag).await;
    settings(&ctx, per_pair).await;
    let p = Parts::new(ctx.clone());
    let r = rec();
    let pairs: Vec<Entry> = PAIRS.iter().map(|a| v2(&r, a, true)).collect();
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(pairs[0].usdt),
    }]);
    p.tokens.remember_decimals(pairs[0].usdt, 18);
    for e in &pairs {
        p.registry.put(e.clone());
    }
    let mut st = shallow_build::State::default();
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    (ctx, p, pairs, st)
}

#[tokio::test]
async fn admit_published_only_on_change_and_hysteresis_holds() {
    let (ctx, p, mut pairs, mut st) = setup("t188-seq", 1).await;
    let pair = (pairs[0].coin, pairs[0].usdt);
    let memo = p.registry.shallow_memo().clone();
    let chosen = pairs[1].pool.reference.clone();
    assert_eq!(by_snap(&p, &pairs), vec![chosen.clone()]);
    let s1 = memo.seq();
    // (а) Обмены в невыбранном пуле: отпечатки пересчитаны, допуск тот же — снимок допуска не публикуется.
    let big = U256::from(300u64) * U256::from(10u64).pow(U256::from(18u8));
    let e = pairs[0].clone();
    let (bought, after) = pool_after::apply(&ctx, &e, e.usdt, big).unwrap();
    pairs[0].pool.state = after;
    p.registry.put(pairs[0].clone());
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    let e = pairs[0].clone();
    let (_, back) = pool_after::apply(&ctx, &e, e.coin, bought.amount_out).unwrap();
    pairs[0].pool.state = back;
    p.registry.put(pairs[0].clone());
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    assert_eq!(memo.seq(), s1, "допуск не сменился — публикации нет");
    let slot = memo.stamps().get(&pair).expect("отпечаток пары");
    assert_eq!(slot.stamp.pools, p.registry.pair_stamp(pair.0, pair.1));
    assert_eq!(
        *shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair),
        vec![chosen.clone()]
    );
    // (б) Выбранный пул стал недостоверным — допуск сменился, снимок опубликован.
    let mut off = pairs[1].clone();
    off.valid = false;
    p.registry.put(off);
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    let s2 = memo.seq();
    assert!(s2 > s1, "смена допуска — публикация");
    let next = by_snap(&p, &pairs);
    assert_eq!(next, direct(&ctx, &p, pair, 1));
    assert_eq!(next.len(), 1);
    assert_ne!(next[0], chosen);
    // (г) Пул вернулся (тот же уровень, меньший адрес): без гистерезиса прямой расчёт выбрал бы его, отбор снимка — нет.
    p.registry.put(pairs[1].clone());
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    assert_eq!(direct(&ctx, &p, pair, 1), vec![chosen.clone()]);
    assert_eq!(by_snap(&p, &pairs), next, "выбранный остаётся");
    assert_eq!(memo.seq(), s2, "допуск не мигает — публикации нет");
    for _ in 0..2 {
        let e = pairs[1].clone();
        let (bought, after) = pool_after::apply(&ctx, &e, e.usdt, big).unwrap();
        pairs[1].pool.state = after;
        p.registry.put(pairs[1].clone());
        shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
        let e = pairs[1].clone();
        let (_, back) = pool_after::apply(&ctx, &e, e.coin, bought.amount_out).unwrap();
        pairs[1].pool.state = back;
        p.registry.put(pairs[1].clone());
        shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
        assert_eq!(by_snap(&p, &pairs), next);
        assert_eq!(memo.seq(), s2);
    }
    assert_eq!(
        *shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair),
        next
    );
}

/// (в) Пара со встречным не USDT. Те же четыре пула записи t178: пулы 0 и 1 — пара «C — T» (монета — токен0), пулы 2 и
/// 3 — пара «T — C» (монета — токен1); «USDT» теста — токен C (его знаки 18 — условие теста: на равенство снимка и
/// прямого расчёта и на отпечаток не влияют). У пары «C — T» встречный T не USDT, курс — по пулам «T — C».
#[tokio::test]
async fn non_usdt_counter_recomputed_by_rate_marks() {
    let ctx = native_common::ctx("t188-rate").await;
    settings(&ctx, 4).await;
    let p = Parts::new(ctx.clone());
    let r = rec();
    let ents: Vec<Entry> = PAIRS
        .iter()
        .enumerate()
        .map(|(i, a)| v2(&r, a, i < 2))
        .collect();
    let (c, t) = (ents[0].coin, ents[0].usdt);
    assert_eq!((ents[2].coin, ents[2].usdt), (t, c));
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(c),
    }]);
    p.tokens.remember_decimals(c, 18);
    for e in &ents {
        p.registry.put(e.clone());
    }
    let mut st = shallow_build::State::default();
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    let pair = (c, t);
    let memo = p.registry.shallow_memo().clone();
    let slot = memo.stamps().get(&pair).expect("отпечаток пары «C — T»");
    assert!(
        !slot.stamp.rate.is_empty(),
        "встречный не USDT — курс в отпечатке"
    );
    assert_eq!(slot.stamp.rate, p.registry.pair_stamp(t, c));
    assert_eq!(by_snap(&p, &ents[..2]), sorted(direct(&ctx, &p, pair, 4)));
    // Пул «T — C» (курс пары «C — T») стал недостоверным: проход по отметкам пересчитывает и пару «C — T».
    let mut off = ents[2].clone();
    off.valid = false;
    p.registry.put(off);
    shallow_build::pass(&ctx, &p.registry, &p.tokens, &mut st);
    let slot2 = memo.stamps().get(&pair).expect("отпечаток пары «C — T»");
    assert_ne!(slot2.stamp.rate, slot.stamp.rate);
    assert_eq!(slot2.stamp.rate, p.registry.pair_stamp(t, c));
    assert_eq!(by_snap(&p, &ents[..2]), sorted(direct(&ctx, &p, pair, 4)));
    assert_eq!(
        sorted(shallow_pick::pick(&ctx, &p.registry, &p.tokens, 18, pair).to_vec()),
        sorted(direct(&ctx, &p, pair, 4))
    );
}

#[test]
fn hot_core_paths_lock_only_the_minimum() {
    // σ плана на сите: под общим замком рядов — только measure_key.
    let plan = src("src/core/sigma_plan.rs");
    let shared = body(&plan, "pub fn entry_shared(");
    let locked: Vec<&str> = shared.lines().filter(|l| l.contains("lk(")).collect();
    assert_eq!(locked.len(), 1, "{shared}");
    assert!(locked[0].contains("measure_key("), "{}", locked[0]);
    for bad in ["format!", ".integer(", ".decimal(", "read(", "key("] {
        let rest = locked[0].replace("measure_key(", "");
        assert!(
            !rest.contains(bad),
            "под замком рядов: «{bad}» в {}",
            locked[0]
        );
    }
    let decide = body(&plan, "fn decide(");
    assert!(!decide.contains("lk("), "решение — вне замка");
    let vol = src("src/core/vol.rs");
    let mk = body(&vol, "pub fn measure_key(");
    for bad in ["format!", ".clone()", "to_string"] {
        assert!(!mk.contains(bad), "measure_key: «{bad}»");
    }
    assert!(
        src("src/core/dirctx.rs").contains("sigma_plan::entry_shared(&core.inner.vol"),
        "сито — через entry_shared"
    );
    // Очередь расчёта: атомарный быстрый путь до замка `rides`.
    let ride = src("src/core/coin_ride.rs");
    let bn = body(&ride, "pub fn blocks_new(");
    let fast = bn.find("rides_live").expect("быстрый путь blocks_new");
    let slow = bn.find("lk(&core.inner.rides)").expect("замок rides");
    assert!(fast < slow, "rides_live — до замка rides");
    for bad in ["format!", "top_up_on("] {
        if let Some(i) = bn.find(bad) {
            assert!(i > slow, "blocks_new: «{bad}» до замка rides");
        }
    }
}
