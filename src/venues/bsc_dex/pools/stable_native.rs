//! Файл: venues/bsc_dex/pools/stable_native.rs
//! Что делает: нативный BNB в пулах PancakeSwap StableSwap и Lista StableSwap (решение «122 X (нативный BNB в
//! стейблсвапах)»). У такого пула одна из монет — метка BNB_ADDRESS 0xEeee… (не контракт ERC-20: decimals() у неё нет,
//! вход — msg.value, выход — call{value}); бот работает только с токенами, поэтому в паре пула и в графе метка — это
//! обёртка dex.native_wrapped (WBNB), а индекс метки хранится в состоянии (StableData.native) — по нему шаг
//! исполнителя строится видом STABLE_U256_BNB (WBNB.withdraw → exchange{value} / BNB → WBNB.deposit).
//! Здесь — только сопоставление «метка ↔ обёртка»: монеты пула в монеты пары, индекс монеты пары в пуле, пара для
//! вопроса реестру фабрики (фабрика хранит пул под меткой). Без сети.
//! Кто вызывает: pools/{load_stable_u, verify_stable_u, factory_calls}.rs, exec/executor/kind.rs (константа).
//! От чего зависит: ctx.rs (dex.native_wrapped).
//! Настройки: dex.native_wrapped [bsc_dex] (не задана — пул с меткой не берётся: сопоставить её не с чем).
//!
//! Источники (официальные, решение 23): pancake-smart-contracts@cb079908 PancakeStableSwapTwoPool.sol L39
//! (BNB_ADDRESS), L127-L140 (initialize: у метки decimals = 18, support_BNB), L457-L464 (exchange: вход BNB —
//! `dx == msg.value`), L644-L663 (transfer_out: `msg.sender.call{gas: bnb_gas, value}`); то же — ThreePool L39, L130,
//! L458, L645-L661; lista-dao/moolah@eb7d73a src/dex/StableSwapPool.sol L54, L125-L138, L507-L514, L697-L716;
//! фабрики хранят пару по отсортированным адресам монет пула — с меткой (PancakeStableSwapFactory.sol L57-L60, L194-L201;
//! StableSwapFactory.sol L52-L55, L206-L210).

use alloy_primitives::Address;

use super::super::ctx::Ctx;

/// BNB_ADDRESS пулов PancakeSwap Stable и Lista — константа протокола (PancakeStableSwapTwoPool.sol L39,
/// StableSwapPool.sol L54), не настройка.
pub const NATIVE: Address =
    alloy_primitives::address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE");

/// decimals метки — 18 (initialize пулов: `if (_coins[i] == BNB_ADDRESS) coinDecimal = 18`, TwoPool L130-L132,
/// Lista L128-L130); decimals() у метки не вызывается — кода по адресу нет.
pub const NATIVE_DECIMALS: u8 = 18;

/// Обёртка нативной монеты: dex.native_wrapped. None — не задана. Ошибок нет.
pub fn wrapped(ctx: &Ctx) -> Option<Address> {
    ctx.addr("dex.native_wrapped").ok()
}

/// Монеты пула → монеты пары (метка → обёртка). Принимает: монеты пула по порядку, обёртку.
/// Возвращает: список той же длины или причину: у пула и метка, и обёртка (пара неоднозначна — такой пул не берётся),
/// метка есть, а dex.native_wrapped не задана.
pub fn pair_coins(coins: &[Address], wrapped: Option<Address>) -> Result<Vec<Address>, String> {
    if !coins.contains(&NATIVE) {
        return Ok(coins.to_vec());
    }
    let Some(w) = wrapped else {
        return Err("у пула нативная монета 0xEeee… (BNB_ADDRESS), а dex.native_wrapped не задана — сопоставить её не с чем".into());
    };
    if coins.contains(&w) {
        return Err(format!(
            "у пула и нативная монета 0xEeee…, и её обёртка {w:#x} — пара в графе была бы неоднозначной, пул не берётся"
        ));
    }
    Ok(coins
        .iter()
        .map(|c| if *c == NATIVE { w } else { *c })
        .collect())
}

/// Индекс метки среди монет пула. Ошибок нет.
pub fn native_index(coins: &[Address]) -> Option<u8> {
    coins.iter().position(|c| *c == NATIVE).map(|p| p as u8)
}

/// Пара для вопроса реестру фабрики PancakeSwap Stable / Lista: обёртка в паре → метка (пул с BNB фабрика хранит под
/// меткой). Принимает: монету, встречный токен, обёртку. Возвращает: другую пару или None (обёртки в паре нет / не
/// задана). Ошибок нет.
pub fn registry_pair(
    a: Address,
    b: Address,
    wrapped: Option<Address>,
) -> Option<(Address, Address)> {
    let w = wrapped?;
    let sub = |x: Address| if x == w { NATIVE } else { x };
    (a == w || b == w).then(|| (sub(a), sub(b)))
}
