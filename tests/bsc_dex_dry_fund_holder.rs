//! Файл: tests/bsc_dex_dry_fund_holder.rs
//! Что делает: чистая логика запасного источника пополнения в симуляции (решение 302, задача t133;
//! exec/dry_fund_holder.rs): держатель пула по виду (holder), крючок обмена (swap_hook), годность держателя при
//! маршруте (holder → route_ok), выбор по балансу (choose).
//! Записанные данные: Uniswap V4 PoolManager BSC 0x28e2ea09…e9df (tests/data/ws_logs_2026-10-26/responses_mgr.json,
//! решение 79), пул GAIB Uniswap V4 0x5c1b4d20…fbbbf2 и сумма перевода 3460500000000000000000 — журнал предпроверки живого
//! бота 15.11.2026 (20:55:04–21:36:49 UTC). Есть ли у пула GAIB крючок обмена — НЕ установлено (docs/TASKS.md,
//! решение 302): в тестах оба случая.
//! Адрес Vault Infinity в записях с подтверждённой ролью нет — VAULT, HOOK и OTHER ниже — условные адреса для проверки
//! чистой логики (роль им не приписывается). Балансы держателей — граница записанной суммы (сумма и сумма ± 1):
//! записанной eth_simulateV1 / balanceOf держателя для GAIB нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::dry_fund_holder, pools::families::Proto, core::types.

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256, address, b256};

use arb::core::types::{HookGroup, HookInfo, PoolRef};
use arb::venues::bsc_dex::exec::dry_fund_holder::{Site, choose, holder, route_ok, swap_hook};
use arb::venues::bsc_dex::pools::families::Proto;

const V4_MANAGER: Address = address!("28e2ea090877bf75740558f6bfb36a5ffee9e9df");
const GAIB_POOL: B256 = b256!("5c1b4d202437dabcacbee41d382dd0f5fa281c0f58c6bfb8b36afd3625fbbbf2");
/// Условные адреса чистой логики (не записи): «значение dex.vault», адрес крючка, второй держатель.
const VAULT: Address = address!("1111111111111111111111111111111111111111");
const HOOK: Address = address!("2222222222222222222222222222222222222222");
const OTHER: Address = address!("3333333333333333333333333333333333333333");

fn need() -> U256 {
    U256::from_str("3460500000000000000000").unwrap()
}

fn gaib_ref() -> PoolRef {
    PoolRef::Singleton {
        manager: V4_MANAGER,
        id: GAIB_POOL,
    }
}

fn hook(before_swap: bool) -> HookInfo {
    HookInfo {
        address: HOOK,
        group: HookGroup::B,
        dynamic_fee: false,
        before_swap,
        after_swap: false,
        before_swap_returns_delta: false,
        after_swap_returns_delta: false,
        raw_bits: 0,
    }
}

/// Пул GAIB маршрута: держатель — через holder(); крючки — None (не прочитаны) / без крючка обмена / с ним.
fn gaib_site(hooks: Option<&HookInfo>) -> Site {
    Site {
        holder: holder(Proto::UniswapV4, &gaib_ref(), None),
        delta_math: true,
        swap_hook: swap_hook(hooks),
        label: "uniswap-v4-bsc GAIB".into(),
    }
}

#[test]
fn holder_by_kind() {
    // V4: держатель — сам PoolManager (dex.vault не нужен).
    assert_eq!(
        holder(Proto::UniswapV4, &gaib_ref(), None),
        Some(V4_MANAGER)
    );
    // Infinity CL: токены в Vault — без dex.vault держатель не известен; с ним — значение dex.vault, не менеджер.
    let inf = PoolRef::Singleton {
        manager: OTHER,
        id: GAIB_POOL,
    };
    assert_eq!(holder(Proto::InfinityCl, &inf, None), None);
    assert_eq!(holder(Proto::InfinityCl, &inf, Some(VAULT)), Some(VAULT));
    // Infinity Stable: ссылка — адрес крючка (pools/verify_inf_stable.rs), держатель — Vault (dex.vault).
    let stable = PoolRef::Contract(HOOK);
    assert_eq!(
        holder(Proto::InfinityStable, &stable, Some(VAULT)),
        Some(VAULT)
    );
    assert_eq!(holder(Proto::InfinityStable, &stable, None), None);
    // Пул со своим контрактом держит свои токены.
    assert_eq!(
        holder(Proto::V2, &PoolRef::Contract(OTHER), None),
        Some(OTHER)
    );
}

#[test]
fn swap_hook_flags() {
    assert_eq!(swap_hook(None), None);
    assert_eq!(swap_hook(Some(&hook(false))), Some(false));
    assert_eq!(swap_hook(Some(&hook(true))), Some(true));
    let mut zero = hook(true);
    zero.address = Address::ZERO;
    assert_eq!(swap_hook(Some(&zero)), Some(false));
}

#[test]
fn gaib_route_same_singleton() {
    let no_hook = hook(false);
    let with_hook = hook(true);
    // Продажа GAIB в тот же PoolManager: V4 без крючка обмена — держатель годен.
    assert!(route_ok(V4_MANAGER, &[gaib_site(Some(&no_hook))]).is_ok());
    // С крючком обмена — нет (крючок мог бы читать баланс держателя).
    let e = route_ok(V4_MANAGER, &[gaib_site(Some(&with_hook))]).unwrap_err();
    assert!(e.contains("крючком обмена"), "{e}");
    // Крючки не прочитаны — нет.
    let e = route_ok(V4_MANAGER, &[gaib_site(None)]).unwrap_err();
    assert!(e.contains("не прочитаны"), "{e}");
    // Держатель пула маршрута не известен — отказ.
    let unknown = Site {
        holder: None,
        ..gaib_site(Some(&no_hook))
    };
    assert!(route_ok(V4_MANAGER, &[unknown]).is_err());
}

#[test]
fn infinity_stable_in_route_blocks_vault() {
    // Реальный путь: holder() для Infinity Stable (ссылка — крючок, hooks = None, как verify_inf_stable.rs) → Vault.
    let stable = Site {
        holder: holder(Proto::InfinityStable, &PoolRef::Contract(HOOK), Some(VAULT)),
        delta_math: false,
        swap_hook: swap_hook(None),
        label: "pancake-infinity-stable".into(),
    };
    let e = route_ok(VAULT, &[stable.clone()]).unwrap_err();
    assert!(e.contains("не только по своему состоянию"), "{e}");
    // Источник — другой держатель (V4 PoolManager): пул Infinity Stable в Vault ему не мешает.
    assert!(route_ok(V4_MANAGER, &[stable]).is_ok());
}

#[test]
fn choose_by_balance() {
    let no_hook = hook(false);
    let route = [gaib_site(Some(&no_hook))];
    assert!(choose(&[], &route, need()).is_err());
    // Ровно сумма — годен; на 1 меньше — нет, причина с цифрами.
    assert_eq!(
        choose(&[(V4_MANAGER, need())], &route, need()),
        Ok(V4_MANAGER)
    );
    let e = choose(&[(V4_MANAGER, need() - U256::from(1u8))], &route, need()).unwrap_err();
    assert!(e.contains("3460500000000000000000"), "{e}");
    // Из двух годных — с наибольшим балансом; негодный при маршруте пропускается.
    let two = [(VAULT, need()), (V4_MANAGER, need() + U256::from(1u8))];
    assert_eq!(choose(&two, &route, need()), Ok(V4_MANAGER));
    let with_hook = hook(true);
    let hooked = [gaib_site(Some(&with_hook))];
    assert_eq!(choose(&two, &hooked, need()), Ok(VAULT));
}
