//! Файл: tests/bsc_dex_dry_fund_holder.rs
//! Что делает: чистая логика запасного источника пополнения в симуляции (решение 302, задача t133;
//! exec/dry_fund_holder.rs): держатель пула по виду, крючок обмена, годность держателя при маршруте, выбор по балансу.
//! Данные — записанные: адреса менеджеров BSC — Uniswap V4 PoolManager 0x28e2ea09…e9df и Infinity CLPoolManager
//! 0xa0ffb9c1…058b (tests/data/ws_logs_2026-10-26/responses_mgr.json, решение 79); пул GAIB Uniswap V4
//! 0x5c1b4d20…fbbbf2 и сумма перевода 3460500000000000000000 — журнал предпроверки живого бота 15.11.2026
//! (20:55:04–21:36:49 UTC, «нужно 3460500000000000000000»). Балансы держателей в тесте — граница этой суммы (сумма и
//! сумма − 1): записанной eth_simulateV1 / balanceOf держателя для GAIB нет (что записать — docs/TASKS.md, решение 302).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::dry_fund_holder, pools::families::Proto, core::types.

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256, address, b256};

use arb::core::types::{HookGroup, HookInfo, PoolRef};
use arb::venues::bsc_dex::exec::dry_fund_holder::{Site, choose, holder, route_ok, swap_hook};
use arb::venues::bsc_dex::pools::families::Proto;

const V4_MANAGER: Address = address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df");
const CL_MANAGER: Address = address!("a0ffb9c1ce1fe56963b0321b32e7a0302114058b");
const GAIB_POOL: B256 = b256!("5c1b4d202437dabcacbee41d382dd0f5fa281c0f58c6bfb8b36afd3625fbbbf2");

fn need() -> U256 {
    U256::from_str("3460500000000000000000").unwrap()
}

fn gaib_ref() -> PoolRef {
    PoolRef::Singleton {
        manager: V4_MANAGER,
        id: GAIB_POOL,
    }
}

/// Пул GAIB маршрута (V4, без крючка — как в журнале, «прямой, исполним как есть»; крючок задаётся флагом).
fn gaib_site(swap_hook: bool) -> Site {
    Site {
        holder: holder(Proto::UniswapV4, &gaib_ref(), None),
        delta_math: true,
        swap_hook,
        label: "uniswap-v4-bsc GAIB".into(),
    }
}

fn hook(before_swap: bool) -> HookInfo {
    HookInfo {
        address: CL_MANAGER, // любой ненулевой адрес из записи
        group: HookGroup::B,
        dynamic_fee: false,
        before_swap,
        after_swap: false,
        before_swap_returns_delta: false,
        after_swap_returns_delta: false,
        raw_bits: 0,
    }
}

#[test]
fn holder_by_kind() {
    // V4: держатель — сам PoolManager (dex.vault не нужен).
    assert_eq!(
        holder(Proto::UniswapV4, &gaib_ref(), None),
        Some(V4_MANAGER)
    );
    // Infinity CL: токены в Vault — без dex.vault держатель не известен; с ним — Vault, а не CLPoolManager.
    let inf = PoolRef::Singleton {
        manager: CL_MANAGER,
        id: GAIB_POOL,
    };
    assert_eq!(holder(Proto::InfinityCl, &inf, None), None);
    assert_eq!(
        holder(Proto::InfinityCl, &inf, Some(V4_MANAGER)),
        Some(V4_MANAGER)
    );
    // Пул со своим контрактом держит свои токены.
    assert_eq!(
        holder(Proto::V2, &PoolRef::Contract(CL_MANAGER), None),
        Some(CL_MANAGER)
    );
}

#[test]
fn swap_hook_flags() {
    assert!(!swap_hook(None));
    assert!(!swap_hook(Some(&hook(false))));
    assert!(swap_hook(Some(&hook(true))));
    let mut zero = hook(true);
    zero.address = Address::ZERO;
    assert!(!swap_hook(Some(&zero)));
}

#[test]
fn gaib_route_same_singleton() {
    // Продажа GAIB в тот же PoolManager: V4 без крючка обмена — держатель годен.
    assert!(route_ok(V4_MANAGER, &[gaib_site(false)]).is_ok());
    // С крючком обмена — нет (крючок мог бы читать баланс держателя).
    let e = route_ok(V4_MANAGER, &[gaib_site(true)]).unwrap_err();
    assert!(e.contains("крючком обмена"), "{e}");
    // Пул Infinity Stable в том же держателе — вид считает не только по своему состоянию.
    let stable = Site {
        holder: Some(V4_MANAGER),
        delta_math: false,
        swap_hook: false,
        label: "stable".into(),
    };
    assert!(route_ok(V4_MANAGER, &[stable.clone()]).is_err());
    // Тот же пул у ДРУГОГО держателя источнику не мешает.
    assert!(route_ok(CL_MANAGER, &[stable]).is_ok());
    // Держатель пула маршрута не известен — отказ.
    let unknown = Site {
        holder: None,
        ..gaib_site(false)
    };
    assert!(route_ok(V4_MANAGER, &[unknown]).is_err());
}

#[test]
fn choose_by_balance() {
    let route = [gaib_site(false)];
    assert!(choose(&[], &route, need()).is_err());
    // Ровно сумма — годен; на 1 меньше — нет, причина с цифрами.
    assert_eq!(
        choose(&[(V4_MANAGER, need())], &route, need()),
        Ok(V4_MANAGER)
    );
    let e = choose(&[(V4_MANAGER, need() - U256::from(1u8))], &route, need()).unwrap_err();
    assert!(e.contains("3460500000000000000000"), "{e}");
    // Из двух годных — с наибольшим балансом; негодный при маршруте пропускается.
    let two = [(CL_MANAGER, need()), (V4_MANAGER, need() + U256::from(1u8))];
    assert_eq!(choose(&two, &route, need()), Ok(V4_MANAGER));
    let hooked = [gaib_site(true)];
    assert_eq!(choose(&two, &hooked, need()), Ok(CL_MANAGER));
}
