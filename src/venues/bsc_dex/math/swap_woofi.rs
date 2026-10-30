//! Файл: venues/bsc_dex/math/swap_woofi.rs
//! Что делает: математика свопа WOOFi Swap V2 (sPMM, решение 80 — экзотическая математика WooFi «по вкладу
//! ликвидности») — порт двух ценовых функций официального контракта WOOFi WooPPV2.sol на целых U256 с теми же
//! округлениями, что в Solidity 0.8.14. Цена берётся НЕ из резервов пула, а от оракула WooracleV2: p (price), s
//! (spread), c (coeff) — это ВХОДЫ, задаются вызывающим (в фазе B читаются с оракула). Два направления:
//! `calc_quote_amount_sell_base` (отдать base, получить quote — `_calcQuoteAmountSellBase`) и
//! `calc_base_amount_sell_quote` (отдать quote, получить base — `_calcBaseAmountSellQuote`). Поверх них — чистый выход
//! свопа после комиссии пула: `sell_base`/`sell_quote` повторяют `_tryQuerySellBase`/`_tryQuerySellQuote` (и равны
//! нетто-выходу `_sellBase`/`_sellQuote`): у sellBase комиссия снимается с ВЫХОДНОГО quote, у sellQuote — с ВХОДНОГО
//! quote до расчёта base. Цена масштабируется десятичными (priceDec = 10^oracle.decimals(base), обычно 8; quoteDec =
//! 10^decimals(quote); baseDec = 10^decimals(base)) — приходят в `Decs`.
//! ВАЖНО (решение 23): Solidity 0.8.14 проверяет всю арифметику (revert при переполнении/вычитании ниже нуля/делении
//! на ноль) — unchecked-блоков в этих функциях нет. Поэтому порт целиком на checked_*: где контракт откатился бы
//! (coef ушёл ниже нуля — сумма/цена слишком велика, p = 0, переполнение) — `None`.
//! Это уровень цены ОДНОГО свопа в одном WooPP-пуле (base↔quote). Направление base→base (через quote) — композиция
//! двух вызовов, её (и общий спред/комиссию из `_swapBaseToBase`) собирает котировка фазы B, не эта математика.
//! Чтение оракула/резервов, события, квотер (WooRouterV2.querySwap), семейство и граф — фаза B.
//! Кто вызывает: quote/pool_quote.rs (через фазу B); тесты (tests/bsc_dex_woofi_math.rs).
//! От чего зависит: alloy-primitives (U256). Настройки: нет (p, s, c, десятичные — с оракула/контракта в фазе B).
//!
//! Источники (официальные, решение 23): woo-finance/WooPoolV2 (HEAD d355ca863bbc0db967a58778ea17c6cc9a1675dd):
//! `_calcQuoteAmountSellBase` / `_calcBaseAmountSellQuote` / `_tryQuerySellBase` / `_tryQuerySellQuote` —
//! contracts/WooPPV2.sol; формат state (price/spread/coeff) и десятичные — contracts/interfaces/IWooracleV2.sol
//! (State), contracts/wooracle/WooracleV2.sol (coeff decimal 18, spread decimal 18, price как chainlink decimal 8),
//! DecimalInfo (priceDec/quoteDec/baseDec) — WooPPV2.decimalInfo.

use alloy_primitives::U256;

/// Единица фикс-точки WooFi (1e18): spread и coeff в 18 знаках, базовый множитель coef. Ошибок нет.
fn one() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// Делитель комиссии пула: feeRate задан «1 в 1e5» (10 = 1 б.п. = 0,01%). Ошибок нет.
fn fee_base() -> U256 {
    U256::from(100000u32)
}

/// Состояние оракула WooracleV2 (IWooracleV2.State) для расчёта цены свопа. price — цена base в quote в `price_dec`
/// знаках (как chainlink, обычно 8); spread (s) и coeff (c) — в 18 знаках. woFeasible проверяет вызывающий (фаза B):
/// неосуществимый оракул = нет цены = нет сделки, до математики не доходит.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WooState {
    pub price: U256,
    pub spread: U256,
    pub coeff: U256,
}

/// Десятичные множители (WooPPV2.DecimalInfo): price_dec = 10^oracle.decimals(base) (обычно 1e8); quote_dec =
/// 10^decimals(quote); base_dec = 10^decimals(base). Приходят с контракта/оракула в фазе B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decs {
    pub price_dec: U256,
    pub quote_dec: U256,
    pub base_dec: U256,
}

/// WooPPV2._calcQuoteAmountSellBase: отдать `base_amount` базовой, получить quote (ДО комиссии пула). Возвращает
/// (quoteAmount, newPrice) — newPrice оракул «постит» после свопа (нужна для последовательных свопов/цикла). None —
/// контракт откатился бы (coef ниже нуля: amount/цена слишком велики; переполнение; price = 0 внутри newPrice).
/// Формула: quote = base·price·(1 − c·base·price − s); newPrice = price·(1 − 2·c·price·base).
pub fn calc_quote_amount_sell_base(
    base_amount: U256,
    s: &WooState,
    d: &Decs,
) -> Option<(U256, U256)> {
    // coef = 1e18 − (coeff·baseAmount·price / baseDec / priceDec) − spread
    let drop = s
        .coeff
        .checked_mul(base_amount)?
        .checked_mul(s.price)?
        .checked_div(d.base_dec)?
        .checked_div(d.price_dec)?;
    let coef = one().checked_sub(drop)?.checked_sub(s.spread)?;
    // quoteAmount = (baseAmount·quoteDec·price / priceDec)·coef / 1e18 / baseDec
    let quote_amount = base_amount
        .checked_mul(d.quote_dec)?
        .checked_mul(s.price)?
        .checked_div(d.price_dec)?
        .checked_mul(coef)?
        .checked_div(one())?
        .checked_div(d.base_dec)?;
    // newPrice = (1e18 − 2·coeff·price·baseAmount / priceDec / baseDec)·price / 1e18
    let np_drop = U256::from(2u8)
        .checked_mul(s.coeff)?
        .checked_mul(s.price)?
        .checked_mul(base_amount)?
        .checked_div(d.price_dec)?
        .checked_div(d.base_dec)?;
    let new_price = one()
        .checked_sub(np_drop)?
        .checked_mul(s.price)?
        .checked_div(one())?;
    Some((quote_amount, new_price))
}

/// WooPPV2._calcBaseAmountSellQuote: отдать `quote_amount` котируемой, получить base (комиссия пула уже снята
/// вызывающим — см. `sell_quote`). Возвращает (baseAmount, newPrice). None — как выше (coef ниже нуля; price = 0;
/// переполнение). Формула: base = quote/price·(1 − c·quote − s); newPrice = price·(1 + 2·c·quote).
pub fn calc_base_amount_sell_quote(
    quote_amount: U256,
    s: &WooState,
    d: &Decs,
) -> Option<(U256, U256)> {
    if s.price.is_zero() {
        return None; // деление на price откатилось бы
    }
    // coef = 1e18 − (quoteAmount·coeff / quoteDec) − spread
    let drop = quote_amount
        .checked_mul(s.coeff)?
        .checked_div(d.quote_dec)?;
    let coef = one().checked_sub(drop)?.checked_sub(s.spread)?;
    // baseAmount = (quoteAmount·baseDec·priceDec / price)·coef / 1e18 / quoteDec
    let base_amount = quote_amount
        .checked_mul(d.base_dec)?
        .checked_mul(d.price_dec)?
        .checked_div(s.price)?
        .checked_mul(coef)?
        .checked_div(one())?
        .checked_div(d.quote_dec)?;
    // newPrice = (1e18·quoteDec + 2·coeff·quoteAmount)·price / quoteDec / 1e18
    let new_price = one()
        .checked_mul(d.quote_dec)?
        .checked_add(
            U256::from(2u8)
                .checked_mul(s.coeff)?
                .checked_mul(quote_amount)?,
        )?
        .checked_mul(s.price)?
        .checked_div(d.quote_dec)?
        .checked_div(one())?;
    Some((base_amount, new_price))
}

/// Чистый выход продажи base за quote с комиссией пула (WooPPV2._tryQuerySellBase, = нетто `_sellBase`): считаем quote
/// по кривой, затем снимаем комиссию с ВЫХОДА. `fee_rate` — «1 в 1e5». Возвращает quote-нетто. None — как у кривой.
pub fn sell_base(base_amount: U256, fee_rate: U256, s: &WooState, d: &Decs) -> Option<U256> {
    let (quote_amount, _) = calc_quote_amount_sell_base(base_amount, s, d)?;
    let fee = quote_amount
        .checked_mul(fee_rate)?
        .checked_div(fee_base())?;
    quote_amount.checked_sub(fee)
}

/// Чистый выход продажи quote за base с комиссией пула (WooPPV2._tryQuerySellQuote, = нетто `_sellQuote`): комиссия
/// снимается с ВХОДНОГО quote ДО расчёта base. `fee_rate` — «1 в 1e5». Возвращает base-нетто. None — как у кривой.
pub fn sell_quote(quote_amount: U256, fee_rate: U256, s: &WooState, d: &Decs) -> Option<U256> {
    let swap_fee = quote_amount
        .checked_mul(fee_rate)?
        .checked_div(fee_base())?;
    let net_quote = quote_amount.checked_sub(swap_fee)?;
    let (base_amount, _) = calc_base_amount_sell_quote(net_quote, s, d)?;
    Some(base_amount)
}
