//! Файл: venues/bsc_dex/math/balancer_v3_buffer.rs
//! Что делает: решение 474 — обмен через буфер ERC4626 хранилища Balancer V3 с точным входом (EXACT_IN): WRAP (базовый
//! токен → обёртка) и UNWRAP (обёртка → базовый токен) так, как их считает Vault.erc4626BufferWrapOrUnwrap: выход =
//! previewDeposit(вход − 1) − 1 / previewRedeem(вход − 1) − 1 обёртки, вход и выход не меньше наименьшей суммы
//! хранилища, буферы не на паузе. Превью — функция самой обёртки (у каждой своя: Aave StataToken, Morpho, …; общего
//! исходника нет), бот берёт её на блоке чтения в одной точке — previewDeposit / previewRedeem опорной суммы
//! (dex.bv3_buffer_preview_raw) — и считает превью суммы x как ⌊x · превью(опорная) / опорная⌋, только для x ≤
//! опорной (больше — котировки нет). Это НЕ побитовая копия превью обёртки (см. «Точность» ниже). Защита перед
//! отправкой сделки: точный ответ квотера сверки — BatchRouter querySwapExactIn с шагами буфера
//! (quote/quoter_balancer_v3_buffer.rs) — и minOut исполнителя (откат всей транзакции, если получено меньше).
//! Предел без внешнего вызова: выход WRAP ≤ обёртки в буфере, выход UNWRAP ≤ базового токена в буфере (Vault.sol L1235,
//! L1356). Больше — хранилище само зовёт deposit / redeem обёртки (выход тот же, но успех вызова обёртки — её лимиты,
//! пауза — бот не считает), поэтому такая сумма котировки не получает.
//! Кто вызывает: math/balancer_v3.rs (amount_out — стороны пары через буфер).
//! От чего зависит: core/types (V3Buffer).
//! Настройки: нет (опорная сумма — pools/load_balancer_v3_buffer.rs).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20241204-v3-vault →
//! contracts/Vault.sol L1139-L1187 (erc4626BufferWrapOrUnwrap: _ensureValidWrapAmount входа L1152 и выхода L1186,
//! предел L1171-L1180), L1193-L1197 (_ensureValidWrapAmount: сумма < _MINIMUM_WRAP_AMOUNT — откат), L1212-L1218 (WRAP
//! EXACT_IN: previewDeposit(amountGiven − 1) − 1), L1235 (буфер покрывает выход — без внешнего вызова), L1333-L1339
//! (UNWRAP EXACT_IN: previewRedeem(amountGiven − 1) − 1), L1356; VaultCommon.sol L188-L198 (буферы на паузе — откат).
//!
//! Точность (ревью t244; исходники конкретных обёрток в этой задаче не сверялись): если превью обёртки линейно —
//! ⌊y · a / b⌋ (по ревью так у OpenZeppelin ERC4626 с виртуальным смещением и у Aave stata: rayDiv / rayMul с
//! округлением вниз), то P = ⌊R · a / b⌋ даёт a / b − 1 / R < P / R ≤ a / b, и для x ≤ R расчёт бота меньше превью
//! обёртки на 0 или 1 единицу токена (никогда не больше — бот не завышает); для x > R ошибка росла бы до ⌈x / R⌉ —
//! поэтому x > R котировки не получает. Для произвольного превью EIP-4626 (нелинейные комиссии и т. п.) граница НЕ
//! гарантирована, возможно и завышение. Курс обёртки на блоке исполнения может отличаться от блока чтения в любую
//! сторону для WRAP и UNWRAP (рост курса уменьшает выход WRAP). Во всех случаях до сделки — точный querySwapExactIn
//! BatchRouter на свежем блоке и minOut исполнителя. Сверка формулы — тест tests/arbitrum_balancer_v3_buffer.rs на
//! записи tools/arbitrum_record/balancer_v3_buffer.py.

use alloy_primitives::U256;

use crate::core::types::pool_vault::V3Buffer;

/// Превью обёртки для суммы по одной точке: ⌊x · за_опорную / опорная⌋. None — опорная 0 или переполнение. Ошибок нет.
fn preview(x: U256, at_ref: U256, reference: U256) -> Option<U256> {
    if reference.is_zero() {
        return None;
    }
    Some(x.checked_mul(at_ref)? / reference)
}

/// Общая часть WRAP / UNWRAP с точным входом. Принимает: буфер, вход, превью опорной суммы в нужную сторону, предел
/// выхода без внешнего вызова. Возвращает: выход. None — буферы на паузе, вход / выход меньше наименьшей суммы, сумма
/// превью (вход − 1) больше опорной (оценка по одной точке не ограничена — шапка, «Точность»), выход больше остатка
/// буфера, переполнение.
fn exact_in(b: &V3Buffer, amount: U256, at_ref: U256, cap: U256) -> Option<U256> {
    if b.paused || amount < b.min_wrap || amount.is_zero() {
        return None;
    }
    // L1217 / L1338: preview(amountGiven − 1) − 1 (проверяемое вычитание: превью 0 — откат хранилища).
    let one = U256::from(1u8);
    let x = amount - one;
    if x > b.reference {
        return None;
    }
    let out = preview(x, at_ref, b.reference)?.checked_sub(one)?;
    // L1186: выход меньше наименьшей суммы — откат; L1235 / L1356: выход больше остатка буфера — внешний вызов обёртки.
    (out >= b.min_wrap && !out.is_zero() && out <= cap).then_some(out)
}

/// WRAP с точным входом: базовый токен → обёртка. Принимает: буфер, вход базового токена (сырые единицы). Возвращает:
/// выход обёртки. None — как exact_in.
pub fn wrap(b: &V3Buffer, amount: U256) -> Option<U256> {
    exact_in(b, amount, b.deposit, b.wrapped_bal)
}

/// UNWRAP с точным входом: обёртка → базовый токен. Принимает: буфер, вход обёртки. Возвращает: выход базового токена.
/// None — как exact_in.
pub fn unwrap(b: &V3Buffer, amount: U256) -> Option<U256> {
    exact_in(b, amount, b.redeem, b.underlying_bal)
}

