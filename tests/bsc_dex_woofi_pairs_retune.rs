//! Файл: tests/bsc_dex_woofi_pairs_retune.rs
//! Что проверяет: решение 305 (ревью В5) — чистая логика справочника без узла и без ответов сети: ключ строки
//! (multi_pair::dir_key) и смена режима (multi_pair::retune) на структурах проекта (directory::Snap, discover::Found).
//! 1. Ключ: DEX вне множества режима — строка пула как есть (без «|»); в множестве — «пул|меньшая|большая», порядок
//!    монет на ключ не влияет.
//! 2. WooFi (woofi_v2, ссылка Member в обоих режимах): «нет → да» — взятая под строкой контракта снимается (и её
//!    статус), взятая под ключом-парой остаётся; «да → нет» — наоборот; «да → нет → да» — после двух шагов остаются
//!    только ключи-пары, дублей одной пары нет.
//! 3. Ellipsis / Nerve в режиме вместе с WooFi: строки развёртки (Snap.multi) и взятые Multi не теряются; DEX, вышедшая
//!    из режима (Ellipsis), теряет свои строки развёртки и взятые Multi, Nerve — нет; пустое множество — развёртка
//!    очищена целиком (как до решения 305).
//! Адреса — из записей узла (tests/data/ellipsis_2026-11-15, nerve_2026-11-15: пул и coins.json) и адрес WooPPV2 на
//! BSC из решения 122 F, 3 / tests/README.md (запись WooFi); ответов сети тест не читает и не подменяет.
//!
//! Кто вызывает: cargo test --test bsc_dex_woofi_pairs_retune.
//! От чего зависит: tests/multi_common (rec, coin), arb::venues::bsc_dex::{directory::Snap, pools::{multi_pair,
//! families, discover::Found}}.

mod multi_common;

use std::collections::HashSet;
use std::str::FromStr;

use alloy_primitives::Address;

use arb::core::types::{DirectoryStatus, PoolRef};
use arb::venues::bsc_dex::directory::Snap;
use arb::venues::bsc_dex::directory::gt_parse::IndexedPool;
use arb::venues::bsc_dex::pools::discover::{Found, sorted};
use arb::venues::bsc_dex::pools::families::{self, Family};
use arb::venues::bsc_dex::pools::multi_pair::{dir_key, retune};

use multi_common::{coin, rec};

/// Общий контракт WooPPV2 на BSC (решение 122 F, 3; tests/README.md, запись WooFi). Адрес площадки из документа
/// проекта, не значение для расчёта.
const WOO_PP: &str = "0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4";

fn addr(s: &str) -> Address {
    Address::from_str(s).unwrap()
}

/// Монета по тикеру из coins.json записи Ellipsis.
fn tok(sym: &str) -> Address {
    coin(&rec("ellipsis_2026-11-15", "coins.json"), sym)
}

/// Пул записи (probe.json → pool).
fn pool_of(dir: &str) -> Address {
    addr(rec(dir, "probe.json")["pool"].as_str().unwrap())
}

/// Взятый пул по ссылке. Ошибок нет.
fn found(f: &'static Family, reference: PoolRef, a: Address, b: Address, c: Address) -> Found {
    let (token0, token1) = sorted(a, b);
    Found {
        family: f,
        reference,
        token0,
        token1,
        coin: c,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    }
}

/// Строка развёртки справочника. Ошибок нет.
fn row(dex: &str, pool: Address, base: Address, quote: Address) -> IndexedPool {
    IndexedPool {
        dex_id: dex.into(),
        pool: format!("{pool:#x}"),
        base,
        quote,
        base_symbol: String::new(),
        quote_symbol: String::new(),
        reserve_usd: None,
    }
}

fn set(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn key_with_and_without_pair() {
    let (pool, a, b) = (format!("{:#x}", addr(WOO_PP)), tok("BTCB"), tok("USDT"));
    let off = dir_key(&set(&[]), "woofi", &pool, a, b);
    assert_eq!(off, pool);
    assert!(!off.contains('|'));
    let on = dir_key(&set(&["woofi"]), "woofi", &pool, a, b);
    assert_eq!(on, dir_key(&set(&["woofi"]), "woofi", &pool, b, a));
    let (lo, hi) = sorted(a, b);
    assert_eq!(on, format!("{pool}|{lo:#x}|{hi:#x}"));
}

#[test]
fn woofi_retune_both_directions() {
    let woo = families::make("woofi", "woofi_v2").unwrap();
    let c = addr(WOO_PP);
    let pool = format!("{c:#x}");
    let usdt = tok("USDT");
    let coins = [tok("BTCB"), tok("USDC")];
    let member = |x: Address| PoolRef::Member { contract: c, coin: x };
    let on = set(&["woofi"]);
    let off = set(&[]);
    // Режим «нет»: одна пара под строкой контракта (так берёт проход без режима).
    let mut s = Snap::default();
    s.taken.insert(pool.clone(), found(woo, member(coins[0]), coins[0], usdt, coins[0]));
    s.status.insert(pool.clone(), DirectoryStatus::Taken);
    retune(&mut s, &off);
    assert_eq!(s.taken.len(), 1, "«нет» → «нет»: ничего не снято");
    // «нет → да»: строка контракта снята вместе со статусом.
    retune(&mut s, &on);
    assert!(s.taken.is_empty() && !s.status.contains_key(&pool));
    // Проход в режиме «да» берёт пары под ключами-парами.
    for &x in &coins {
        let k = dir_key(&on, "woofi", &pool, x, usdt);
        s.taken.insert(k.clone(), found(woo, member(x), x, usdt, x));
        s.status.insert(k, DirectoryStatus::Taken);
    }
    retune(&mut s, &on);
    assert_eq!(s.taken.len(), 2, "«да» → «да»: ничего не снято");
    // «да → нет»: пары с ключом-парой сняты.
    retune(&mut s, &off);
    assert!(s.taken.is_empty() && s.status.is_empty());
    // «нет → да» ещё раз после прохода «нет»: остаются только ключи-пары, каждая пара — одна запись.
    s.taken.insert(pool.clone(), found(woo, member(coins[0]), coins[0], usdt, coins[0]));
    retune(&mut s, &on);
    for &x in &coins {
        let k = dir_key(&on, "woofi", &pool, x, usdt);
        s.taken.insert(k, found(woo, member(x), x, usdt, x));
    }
    assert!(s.taken.keys().all(|k| k.contains('|')));
    let refs: HashSet<PoolRef> = s.taken.values().map(|f| f.reference.clone()).collect();
    assert_eq!(refs.len(), s.taken.len(), "дубль пары во взятых");
}

#[test]
fn multi_coin_rows_kept_and_dropped_by_dex() {
    let ell = families::make("ellipsis", "curve_stableswap").unwrap();
    let ner = families::make("nerve", "saddle_stableswap").unwrap();
    let (pe, pn) = (pool_of("ellipsis_2026-11-15"), pool_of("nerve_2026-11-15"));
    let nerve_coin = |s: &str| coin(&rec("nerve_2026-11-15", "coins.json"), s);
    let (usdt, usdc) = (tok("USDT"), tok("USDC"));
    let (n_usdt, n_usdc) = (nerve_coin("USDT"), nerve_coin("USDC"));
    let multi = |c: Address, a: Address, b: Address| {
        let (token0, token1) = sorted(a, b);
        PoolRef::Multi { contract: c, token0, token1 }
    };
    let mut s = Snap::default();
    let all = set(&["ellipsis", "nerve", "woofi"]);
    let ke = dir_key(&all, "ellipsis", &format!("{pe:#x}"), usdc, usdt);
    let kn = dir_key(&all, "nerve", &format!("{pn:#x}"), n_usdc, n_usdt);
    s.taken.insert(ke.clone(), found(ell, multi(pe, usdc, usdt), usdc, usdt, usdc));
    s.taken.insert(kn.clone(), found(ner, multi(pn, n_usdc, n_usdt), n_usdc, n_usdt, n_usdc));
    s.multi.entry(usdc).or_default().push(row("ellipsis", pe, usdc, usdt));
    s.multi.entry(n_usdc).or_default().push(row("nerve", pn, n_usdc, n_usdt));
    // Все в режиме (и WooFi): ничего не теряется.
    retune(&mut s, &all);
    assert_eq!(s.taken.len(), 2);
    assert_eq!(s.multi.values().map(Vec::len).sum::<usize>(), 2);
    // Ellipsis вышла из режима: её развёртка и взятая Multi сняты, Nerve — на месте.
    retune(&mut s, &set(&["nerve", "woofi"]));
    assert!(!s.taken.contains_key(&ke) && s.taken.contains_key(&kn));
    assert!(s.multi.values().flatten().all(|p| p.dex_id == "nerve"));
    assert_eq!(s.multi.values().map(Vec::len).sum::<usize>(), 1);
    // Пустое множество — развёртка очищена целиком, как до решения 305.
    retune(&mut s, &set(&[]));
    assert!(s.multi.is_empty());
}
