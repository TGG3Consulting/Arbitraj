//! Файл: venues/bsc_dex/send/drop_wait.rs
//! Что делает: решение 437 (1) — окно «выброшена» трекера сети dex.gas_mode = eip1559_l1 (send/track_l1.rs) и его
//! проверки; читается в SendCfg до отправки (решение 398). Ревью 437, п. 7: окно меньше send.queue_max_wait_s.
//! Ревью 439 (1): нужен и порог возраста блока feed.max_block_age_ms меньше окна — «выброшена» только когда блок
//! свежей головы своего узла не старше порога (отставший узел не видит нашу транзакцию в блоке; feed.stale_after_ms
//! ограничивает только момент получения номера, не время блока). Порог меньше окна: время блока головы ≥ часы
//! сервера − порог ≥ отправка + окно − порог > отправки — узел видел блоки после отправки.
//! Кто вызывает: send/tx.rs (SendCfg::read).
//! От чего зависит: ctx.rs, send/gas_mode.rs, node/head_age.rs (имя и чтение порога).
//! Настройки: send.drop_wait_ms, feed.max_block_age_ms [bsc_dex] (только dex.gas_mode = eip1559_l1).

use std::time::Duration;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::head_age;
use super::gas_mode::{self, GasMode};

/// Окно трекера eip1559_l1 (send/track_l1.rs).
#[derive(Debug, Clone, Copy)]
pub struct DropWait {
    /// send.drop_wait_ms — сколько ждать квитанции после отправки, прежде чем по nonce решить «выброшена».
    pub window: Duration,
    /// feed.max_block_age_ms — блок свежей головы своего узла не старше этого (ревью 439 (1)).
    pub max_block_age: Duration,
}

/// Что делает: читает окно для сети eip1559_l1. Модель газа не задана или неверна — None, как legacy (транзакция всё
/// равно не уйдёт — цену газа без модели не посчитать: exec/tx_fee.rs, exec/bundle_send.rs).
/// Принимает: окружение, send.queue_max_wait_s. Возвращает: Some — eip1559_l1; None — legacy (BSC, трекер с отменами).
/// Ошибки: NotConfigured — не задана send.drop_wait_ms или feed.max_block_age_ms, окно не меньше
/// send.queue_max_wait_s (ревью 437, п. 7), порог возраста блока не меньше окна (ревью 439 (1)); Internal (хранилище).
pub fn read(ctx: &Ctx, queue_max_wait: Duration) -> GatewayResult<Option<DropWait>> {
    if !matches!(gas_mode::read(ctx), Ok(GasMode::Eip1559L1)) {
        return Ok(None);
    }
    let window = Duration::from_millis(ctx.int("send.drop_wait_ms")?);
    // Ревью 437, п. 7: окно «выброшена» не короче очереди — иначе итог всегда «исход неизвестен».
    if window >= queue_max_wait {
        let why = format!(
            "send.drop_wait_ms ({} мс) не меньше send.queue_max_wait_s ({} с): итог «выброшена» не наступит \
             раньше остановки очереди — отправка не работает",
            window.as_millis(),
            queue_max_wait.as_secs()
        );
        return Err(ctx.not_set("send.drop_wait_ms", why));
    }
    let Some(age) = head_age::max_age_ms(ctx)? else {
        let why = format!(
            "{} не задана: у dex.gas_mode = eip1559_l1 итог «выброшена» без проверки \
             возраста блока своего узла недостоверен (отставший узел не видит транзакцию \
             в блоке) — отправка не работает",
            head_age::SETTING
        );
        return Err(ctx.not_set(head_age::SETTING, why));
    };
    let max_block_age = Duration::from_millis(age);
    if max_block_age >= window {
        let why = format!(
            "{} ({age} мс) не меньше send.drop_wait_ms ({} мс): узел, отставший на \
             окно, не отличить от выброшенной транзакции — отправка не работает",
            head_age::SETTING,
            window.as_millis()
        );
        return Err(ctx.not_set(head_age::SETTING, why));
    }
    Ok(Some(DropWait {
        window,
        max_block_age,
    }))
}
