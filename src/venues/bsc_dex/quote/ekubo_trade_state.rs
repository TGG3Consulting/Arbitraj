//! Файл: venues/bsc_dex/quote/ekubo_trade_state.rs
//! Что делает: состояние пула Ekubo «к блоку сделки» (решение 454). У пула без TWAMM — прочитанное (событиями ведётся
//! точно). У пула с TWAMM ядро перед нашим обменом исполняет виртуальные ордера до времени блока сделки
//! (TWAMM.beforeSwap), а время блока сделки заранее неизвестно: берутся два времени — t = max(время блока чтения, время
//! нашего сервера, секунды) и t + 1 (тот же приём, что у MEVCapture решения 447: два возможных случая, расчёт — по
//! худшему). Сравнение времени блока с часами сервера — как у проверки возраста головы (node/head_age.rs, решение 259).
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs (ветка Ekubo).
//! От чего зависит: core/types (EkuboData), math/ekubo_virtual.rs, chrono.
//! Настройки: нет (горизонт прочитанного — dex.ekubo_twamm_horizon_s, задаёт чтение пула).
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/extensions/TWAMM.sol L655-L657 (beforeSwap), L394-L600
//! (исполнение до block.timestamp).

use crate::core::types::EkuboData;

use super::super::math::ekubo_virtual::advance;

/// Состояния пула к возможным временам блока сделки. Принимает: прочитанное состояние. Возвращает: одно (без TWAMM)
/// или два состояния. Ошибки (текст): время сделки за прочитанным горизонтом TWAMM или расчёт виртуальных ордеров
/// откатился бы.
pub fn at_trade(d: &EkuboData) -> Result<Vec<EkuboData>, String> {
    if d.twamm.is_none() {
        return Ok(vec![d.clone()]);
    }
    let secs = chrono::Utc::now().timestamp();
    let now = u64::try_from(secs).unwrap_or_default();
    let t = d.block_ts.max(now);
    let mut out = Vec::with_capacity(2);
    for at in [t, t + 1] {
        let s = advance(d, at).ok_or_else(|| {
            format!(
                "TWAMM: виртуальные ордера к времени {at} не считаются — время за \
                 прочитанным горизонтом (dex.ekubo_twamm_horizon_s) или расчёт откатился бы; \
                 пул нужно перечитать"
            )
        })?;
        out.push(s);
    }
    Ok(out)
}
