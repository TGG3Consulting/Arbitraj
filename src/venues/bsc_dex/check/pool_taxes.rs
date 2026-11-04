//! Файл: venues/bsc_dex/check/pool_taxes.rs
//! Что делает: налоги монеты по пулам (решение 146) — чистые функции без обращений к узлу: налоги одного пула в
//! процентах по замеру пробы (check/probe.rs Measured) и налоги монеты для ядра по налогам всех её пулов.
//! Налоги монеты (CoinCheck — пороги tax.max_*_pct, чёрный список, налог перевода для вывода) сохраняют прежний смысл:
//! покупка и продажа — НАИМЕНЬШИЙ среди пулов, где налог этой стороны измерен (монета торгуется, пока есть хоть один
//! чистый пул; в котировке у каждого шага — налог именно его пула, quote/select.rs); перевод от пула не зависит — если
//! замеры разные, берётся наибольший.
//! Там же — итог проверки монеты по пробам всех пулов (outcome: Known / Failed / Wait / Config).
//! Кто вызывает: check/taxes.rs; тесты.
//! От чего зависит: check/probe.rs (Measured), check/state.rs (PoolTaxes), check/taxes.rs (Outcome), check/taxes_util.rs (pct), core/types.
//! Настройки: нет.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6; продажа — по ответу
//! квотера на продажу и приросту встречного токена кошелька за настоящую продажу в симуляции (решение 146).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::TokenTaxes;

use super::probe::Measured;
use super::state::PoolTaxes;
use super::taxes::Outcome;
use super::taxes_util::pct;

/// Налоги монеты в одном пуле. Принимает: замер пробы, decimals монеты и встречного токена пула (USDT или
/// промежуточной). Возвращает: None — делить не на что (ответ квотера на покупку 0 или пробный перевод 0); налог
/// продажи — None, если продажа не измерена (Measured.sell = Err) или ответ квотера на продажу 0. Ошибок нет.
pub fn percent(m: &Measured, coin_dec: u8, counter_dec: u8) -> Option<PoolTaxes> {
    Some(PoolTaxes {
        buy_pct: pct(m.buy_expected, m.buy_got, coin_dec)?,
        sell_pct: m
            .sell
            .as_ref()
            .ok()
            .and_then(|(expected, got)| sell_pct(*expected, *got, counter_dec)),
        transfer_pct: pct(m.transfer_sent, m.transfer_got, coin_dec)?,
    })
}

/// Налог продажи пула, %: (ответ квотера − пришло) / ответ квотера · 100 (пришло больше — 0). Принимает: суммы в
/// единицах встречного токена, его decimals. Возвращает: None — ответ квотера 0 (налог НЕ ИЗМЕРЕН, не 0). Ошибок нет.
pub fn sell_pct(expected: U256, got: U256, counter_dec: u8) -> Option<Decimal> {
    pct(expected, got, counter_dec)
}

/// Налоги монеты для ядра по налогам её пулов. Принимает: налоги пулов с удачной пробой. Возвращает: покупка и
/// продажа — наименьшие среди измеренных, перевод — наибольший; None — пулов нет или налог продажи не измерен ни в
/// одном пуле. Ошибок нет.
pub fn coin(pools: &[PoolTaxes]) -> Option<TokenTaxes> {
    Some(TokenTaxes {
        buy_pct: pools.iter().map(|p| p.buy_pct).min()?,
        sell_pct: pools.iter().filter_map(|p| p.sell_pct).min()?,
        transfer_pct: pools.iter().map(|p| p.transfer_pct).max()?,
    })
}

/// Итог по продаже, когда налог продажи не измерен ни в одном пуле (решение 146). Принимает: причины по пулам
/// (откатилась ли продажа, текст), были ли помехи на других пулах (узел, источник пробы, настройка).
/// Возвращает: Ok(причина) — вердикт «проверка не прошла» (каждая пробная продажа откатилась и помех не было: монету не
/// продать ни в одном пуле); Err(причина) — «не проверено» (повтор). Ошибок нет.
pub fn sell_verdict(misses: &[(bool, String)], interfered: bool) -> Result<String, String> {
    let why = format!(
        "налог продажи не измерен ни в одном пуле — {}",
        misses
            .iter()
            .map(|m| m.1.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    );
    if !misses.is_empty() && !interfered && misses.iter().all(|m| m.0) {
        Ok(why)
    } else {
        Err(why)
    }
}

/// Почему налог продажи пула не измерен — (откатилась ли продажа, «DEX: причина»). Принимает: замер пробы, имя DEX.
/// Ошибок нет.
pub fn miss(m: &Measured, dex: &str) -> (bool, String) {
    let (reverted, why) = match &m.sell {
        Err(x) => (x.reverted, x.why.clone()),
        Ok(_) => (false, "ответ квотера на продажу 0".into()),
    };
    (reverted, format!("{dex}: {why}"))
}

/// Причины проверки по пулам без замера: (вердикты «пробный обмен откатился», помехи, незаданные настройки с именами).
pub type Others = (Vec<String>, Vec<String>, Vec<(String, Option<String>)>);

/// Итог проверки монеты по пробам всех пулов. Принимает: налоги пулов с удачной пробой, был ли пул с выходом 0,
/// причины неизмеренной продажи, причины остальных пулов. Возвращает: Known — налоги по пулам (coin); покупка где-то
/// измерена, продажа нигде — sell_verdict (Failed / Wait); иначе как до решения 146: выход 0 — Failed; настройка —
/// Config; помеха на любом пуле — Wait (вердикт — только если каждый пул дал вердикт); иначе Failed. Ошибок нет.
pub fn outcome(
    measured: &[PoolTaxes],
    zero: bool,
    misses: &[(bool, String)],
    (failed, env, config): Others,
) -> Outcome {
    if let Some(t) = coin(measured) {
        return Outcome::Known(t);
    }
    if !measured.is_empty() {
        let interfered = !env.is_empty() || !config.is_empty();
        return match sell_verdict(misses, interfered) {
            Ok(why) => Outcome::Failed(why),
            Err(why) => Outcome::Wait(why),
        };
    }
    if zero {
        return Outcome::Failed("пробный обмен: расчётный выход 0 — налог не определить".into());
    }
    if !config.is_empty() {
        let why: Vec<String> = config.iter().map(|c| c.0.clone()).collect();
        return Outcome::Config(
            why.join("; "),
            config.into_iter().filter_map(|c| c.1).collect(),
        );
    }
    if !env.is_empty() {
        return Outcome::Wait(env.join("; "));
    }
    Outcome::Failed(failed.join("; "))
}
