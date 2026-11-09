//! Файл: tests/core_venue_dir.rs
//! Что проверяет: решение 242 на живом ядре с SQLite (настоящий Core, настройки в хранилище, виды площадок шлюза
//! bingx и mexc — биржи, bsc_dex — DEX):
//! 1) правило стороны (core/venue_dir.rs rule/of): биржа → DEX — биржа покупки, DEX → биржа — биржа продажи,
//!    биржа → биржа — биржа покупки (buy_on), DEX ↔ DEX (покупка и продажа на одной DEX, как cycle_plan::direction) —
//!    один набор «dex_dex»; вид неизвестен — области нет;
//! 2) лимит на сделку (limits_parts::parts — бюджет сделки и «без потолка»): пустые пары — составляющие ровно прежние;
//!    «BingX · биржа → DEX» действует на BingX → DEX и НЕ действует на DEX → BingX, BingX → MEXC, MEXC → DEX;
//!    «биржа → биржа» — у биржи покупки; «DEX → биржа» — у биржи; снятие значения — снова прежние;
//! 3) пороги прибыли (thresholds: profit.rs, card_max.rs, uncapped.rs, precheck_dex_sell.rs) — так же;
//! 4) доля закрытия (reserve::closure_share) — пара сильнее площадки-получателя, без пары — прежняя площадка B;
//!    k (profit.price_move_k) — пара сильнее монеты, без пары — монета;
//! 5) DEX ↔ DEX: лимит цикла (как cycle_plan.rs: пара, иначе площадка DEX, иначе вся система).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{venue_dir, limits_parts, reserve, dry}, arb::settings. Моков нет.

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::dry::Mode;
use arb::core::limits_parts::{Parts, parts};
use arb::core::reserve;
use arb::core::types::{Asset, Direction, VenueId};
use arb::core::venue_dir::{DirKind, VenueDir, of, rule, thresholds};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::read::At;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use tokio::sync::broadcast;

const CAKE: &str = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82";

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn dir(a: &str, b: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE.parse().unwrap()),
        },
        buy_on: v(a),
        sell_on: v(b),
    }
}

/// Направления проверки: все пары площадок с переводом и цикл DEX ↔ DEX.
fn dirs() -> Vec<Direction> {
    [
        ("bingx", "bsc_dex"),
        ("bsc_dex", "bingx"),
        ("bingx", "mexc"),
        ("mexc", "bingx"),
        ("mexc", "bsc_dex"),
        ("bsc_dex", "mexc"),
        ("bsc_dex", "bsc_dex"),
    ]
    .iter()
    .map(|(a, b)| dir(a, b))
    .collect()
}

/// Настоящее ядро без адаптеров (как tests/core_price_reserve.rs) и виды площадок шлюза.
async fn core(name: &str) -> (Core, Store) {
    let p = std::env::temp_dir().join(format!("arb_vd_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    let db = Db::open(&p.join("arb.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(p.join("secrets.enc")),
        journal: journal.clone(),
    };
    let deps = Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: p.clone(),
    };
    let core = Core::start(deps).await;
    *core.inner.kinds.write().unwrap() = vec![
        (v("bingx"), VenueKind::Cex),
        (v("mexc"), VenueKind::Cex),
        (v("bsc_dex"), VenueKind::Dex),
    ];
    // Решение 242, дополнение: те же виды — хранилищу (проверка пары при записи), как делает Core::start.
    let kinds = core.inner.kinds.read().unwrap().clone();
    settings
        .venue_kinds()
        .set(kinds.into_iter().map(|(id, k)| (id.0, k)));
    (core, settings)
}

async fn set(s: &Store, name: &str, scope: Scope, val: &str) {
    s.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

async fn unset(s: &Store, name: &str, scope: Scope) {
    s.set(name, scope, None, "тест").await.unwrap();
}

fn pair(venue: &str, kind: DirKind) -> Scope {
    VenueDir { venue, kind }.scope()
}

/// Прежние значения: лимиты, пороги, k, доля закрытия.
async fn base(s: &Store) {
    set(s, "money.max_per_trade_usd", Scope::Global, "100").await;
    set(
        s,
        "money.max_per_trade_usd",
        Scope::Venue("bsc_dex".into()),
        "40",
    )
    .await;
    set(s, "money.max_per_coin_usd", Scope::Global, "1000").await;
    set(s, "money.max_in_transit_usd", Scope::Global, "5000").await;
    set(s, "money.min_profit_usd", Scope::Global, "0.3").await;
    set(s, "money.min_profit_pct", Scope::Global, "0.5").await;
    set(s, "profit.price_move_k", Scope::Global, "1.5").await;
    set(s, "profit.price_move_k", Scope::Coin(CAKE.into()), "2").await;
    set(
        s,
        "profit.closure_share",
        Scope::Venue("bingx".into()),
        "0.39",
    )
    .await;
}

/// Всё, что читают места сделки по направлению.
#[derive(Debug, PartialEq)]
struct Read {
    parts: Parts,
    thr: (Decimal, Decimal),
    share: Option<Decimal>,
    k: Decimal,
}

fn read(core: &Core, s: &Store, x: &Direction) -> Read {
    let vd = of(core, x);
    Read {
        parts: parts(core, x, &Mode::Live).unwrap(),
        thr: thresholds(s, vd).unwrap(),
        share: reserve::closure_share(s, x, vd).unwrap(),
        k: s.decimal_dir(
            "profit.price_move_k",
            vd,
            At {
                venue: None,
                coin: Some(CAKE),
            },
        )
        .unwrap(),
    }
}

#[test]
fn side_rule() {
    use VenueKind::{Cex, Dex};
    let x = dir("bingx", "bsc_dex");
    assert_eq!(
        rule(&x, Some(Cex), Some(Dex)),
        Some(VenueDir {
            venue: "bingx",
            kind: DirKind::CexToDex
        })
    );
    let x = dir("bsc_dex", "bingx");
    assert_eq!(
        rule(&x, Some(Dex), Some(Cex)),
        Some(VenueDir {
            venue: "bingx",
            kind: DirKind::DexToCex
        })
    );
    let x = dir("mexc", "bingx");
    assert_eq!(
        rule(&x, Some(Cex), Some(Cex)),
        Some(VenueDir {
            venue: "mexc",
            kind: DirKind::CexCex
        })
    );
    let x = dir("bsc_dex", "bsc_dex");
    let c = rule(&x, Some(Dex), Some(Dex)).unwrap();
    assert_eq!(c.kind, DirKind::DexDex);
    assert_eq!(c.scope(), Scope::VenueDir("dex_dex".into()));
    assert_eq!(rule(&x, None, Some(Dex)), None);
}

#[tokio::test]
async fn empty_is_as_before_and_pair_applies_only_to_its_side() {
    let (core, s) = core("side").await;
    base(&s).await;
    // Пусто: всё как раньше — сравнение с прежним чтением (площадка A / вся система / монета / площадка B).
    let before: Vec<Read> = dirs().iter().map(|x| read(&core, &s, x)).collect();
    for (x, r) in dirs().iter().zip(&before) {
        let at_a = At {
            venue: Some(&x.buy_on.0),
            coin: None,
        };
        let at_b = At {
            venue: Some(&x.sell_on.0),
            coin: None,
        };
        assert_eq!(
            r.parts.per_trade,
            s.decimal("money.max_per_trade_usd", at_a).unwrap()
        );
        assert_eq!(r.thr, (d("0.3"), d("0.5")));
        assert_eq!(r.k, d("2"));
        assert_eq!(r.share, s.decimal("profit.closure_share", at_b).ok());
    }
    let idx = |a: &str, b: &str| {
        dirs()
            .iter()
            .position(|x| x.buy_on.0 == a && x.sell_on.0 == b)
            .unwrap()
    };

    // «BingX · биржа → DEX»: только BingX → DEX.
    let p = pair("bingx", DirKind::CexToDex);
    for n in [
        "money.max_per_trade_usd",
        "money.min_profit_usd",
        "money.min_profit_pct",
        "profit.price_move_k",
        "profit.closure_share",
    ] {
        set(&s, n, p.clone(), "0.7").await;
    }
    for (i, x) in dirs().iter().enumerate() {
        let r = read(&core, &s, x);
        if i == idx("bingx", "bsc_dex") {
            assert_eq!(r.parts.per_trade, d("0.7"));
            assert_eq!(r.thr, (d("0.7"), d("0.7")));
            assert_eq!(r.k, d("0.7"));
            assert_eq!(r.share, Some(d("0.7")));
        } else {
            assert_eq!(r, before[i], "{x:?}");
        }
    }
    for n in [
        "money.max_per_trade_usd",
        "money.min_profit_usd",
        "money.min_profit_pct",
        "profit.price_move_k",
        "profit.closure_share",
    ] {
        unset(&s, n, p.clone()).await;
    }

    // «биржа → биржа» — у биржи покупки: MEXC · биржа → биржа действует на MEXC → BingX, не на BingX → MEXC.
    set(
        &s,
        "money.max_per_trade_usd",
        pair("mexc", DirKind::CexCex),
        "9",
    )
    .await;
    // «DEX → биржа» — у биржи: BingX · DEX → биржа действует на DEX → BingX, не на DEX → MEXC.
    set(
        &s,
        "profit.closure_share",
        pair("bingx", DirKind::DexToCex),
        "0.5",
    )
    .await;
    // DEX ↔ DEX — один набор для цикла.
    set(
        &s,
        "money.max_per_trade_usd",
        Scope::VenueDir("dex_dex".into()),
        "25",
    )
    .await;
    for (i, x) in dirs().iter().enumerate() {
        let r = read(&core, &s, x);
        let mut want = before[i].clone_read();
        if i == idx("mexc", "bingx") {
            want.parts.per_trade = d("9");
        }
        if i == idx("bsc_dex", "bingx") {
            want.share = Some(d("0.5"));
        }
        if i == idx("bsc_dex", "bsc_dex") {
            want.parts.per_trade = d("25");
        }
        assert_eq!(r, want, "{x:?}");
    }
    // Откат: снятие значений — снова ровно прежнее.
    unset(&s, "money.max_per_trade_usd", pair("mexc", DirKind::CexCex)).await;
    unset(&s, "profit.closure_share", pair("bingx", DirKind::DexToCex)).await;
    unset(
        &s,
        "money.max_per_trade_usd",
        Scope::VenueDir("dex_dex".into()),
    )
    .await;
    let after: Vec<Read> = dirs().iter().map(|x| read(&core, &s, x)).collect();
    assert_eq!(after, before);
}

impl Read {
    /// Копия (Parts — Copy, остальное — простые значения).
    fn clone_read(&self) -> Read {
        Read {
            parts: self.parts,
            thr: self.thr,
            share: self.share,
            k: self.k,
        }
    }
}
