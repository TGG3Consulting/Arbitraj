//! Файл: venues/bsc_dex/exec/bundle_notify.rs
//! Что делает: уведомления жизненного цикла мульти-роутерного бандла свопов (решение 84, фаза 80.8c-iii). События
//! видны на экране «Уведомления» и уходят в Telegram, если включены (тот же механизм, что у предторговой верификации,
//! решение 83: MarketEvent::Notice → ядро → notify/dispatch.rs; коды в notify/catalog.rs группой «Бандл свопов»).
//! Отказ предторговой верификации (exec.verify_rejected) НЕ дублируем — он уже шлётся из exec/verify_gate.rs (80.6).
//! Логику исполнения/отправки/очереди НЕ меняет — только отображение: текст каждого события строит чистая функция
//! (*_text — тестируется без сети), а тонкая обёртка эмитит его через Stats.
//! Кто вызывает: exec/bundle_trade.rs (собран), exec/bundle_send.rs (симуляция, отправка, включён/не включён); тесты.
//! От чего зависит: gateway/events.rs (MarketEvent::Notice), send/stats.rs (Stats), ctx.rs (venue); чистые функции — нет.
//! Настройки: нет (включение каждого события — notify.event.* по коду, область «событие»).

use alloy_primitives::B256;

use crate::gateway::events::MarketEvent;

use super::super::ctx::Ctx;
use super::super::send::stats::Stats;

/// Коды событий (notify/catalog.rs, группа «Бандл свопов»).
pub const ASSEMBLED: &str = "bundle.assembled";
pub const SIMULATED: &str = "bundle.simulated";
pub const SIM_REVERTED: &str = "bundle.sim_reverted";
pub const SENT: &str = "bundle.sent";
pub const INCLUDED: &str = "bundle.included";
pub const NOT_INCLUDED: &str = "bundle.not_included";

/// Эмитит событие Notice (код + текст) для площадки DEX. Нет канала событий — ничего (Stats::emit). Ошибок нет.
fn emit(stats: &Stats, ctx: &Ctx, code: &str, text: String) {
    stats.emit(MarketEvent::Notice {
        venue: ctx.venue(),
        code: code.to_string(),
        text,
    });
}

/// Текст «бандл собран»: сколько роутеров (DEX-групп), ног маршрута, транзакций и первый nonce. Чистая. Ошибок нет.
pub fn assembled_text(routers: usize, legs: usize, txs: usize, first_nonce: u64) -> String {
    format!(
        "Бандл свопов собран (решение 84): роутеров {routers}, ног маршрута {legs}, транзакций {txs}, первый nonce {first_nonce}"
    )
}

/// Текст «симуляция прошла»: весь упорядоченный бандл проверен на своём узле (межтранзакционные эффекты, газ, базы
/// BALANCE_CHECK, решение 59 «кто меняет»). Чистая. Ошибок нет.
pub fn simulated_text(routers: usize) -> String {
    format!(
        "Симуляция бандла прошла: {routers} свопов проверены по порядку на своём узле (газ и базы BALANCE_CHECK учтены) — отправляем"
    )
}

/// Текст «симуляция откатилась»: причина (откат шага или чужой пул, решение 59). Чистая. Ошибок нет.
pub fn sim_reverted_text(reason: &str) -> String {
    format!("Симуляция бандла откатилась — не отправлен: {reason}")
}

/// Текст «бандл отправлен»: приватные каналы и предел блока maxBlockNumber (за ним бандл истекает). Чистая. Ошибок нет.
pub fn sent_text(channels: &[String], max_block: u64) -> String {
    let chans = if channels.is_empty() {
        "—".to_string()
    } else {
        channels.join(", ")
    };
    format!("Бандл отправлен приватно (каналы: {chans}), предел блока maxBlockNumber {max_block}")
}

/// Текст «бандл включён»: номер блока (если прочитан) и хеши транзакций по порядку. Чистая. Ошибок нет.
pub fn included_text(hashes: &[B256], block: Option<u64>) -> String {
    let where_ = match block {
        Some(b) => format!("в блок {b}"),
        None => "в блок".to_string(),
    };
    let hs = hashes
        .iter()
        .map(|h| format!("{h:#x}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Бандл включён {where_}: хеши [{hs}]")
}

/// Текст «бандл не попал в блок»: истёк по пределу блока (газ не потрачен, решения 40/45), очередь кошелька свободна —
/// следующая транзакция берёт тот же первый nonce. Чистая. Ошибок нет.
pub fn not_included_text(max_block: u64) -> String {
    format!(
        "Бандл не попал в блок к пределу maxBlockNumber {max_block} — истёк (газ не потрачен), очередь кошелька свободна"
    )
}

/// Бандл собран. Ошибок нет.
pub fn assembled(
    stats: &Stats,
    ctx: &Ctx,
    routers: usize,
    legs: usize,
    txs: usize,
    first_nonce: u64,
) {
    emit(
        stats,
        ctx,
        ASSEMBLED,
        assembled_text(routers, legs, txs, first_nonce),
    );
}

/// Симуляция прошла. Ошибок нет.
pub fn simulated(stats: &Stats, ctx: &Ctx, routers: usize) {
    emit(stats, ctx, SIMULATED, simulated_text(routers));
}

/// Симуляция откатилась. Ошибок нет.
pub fn sim_reverted(stats: &Stats, ctx: &Ctx, reason: &str) {
    emit(stats, ctx, SIM_REVERTED, sim_reverted_text(reason));
}

/// Бандл отправлен. Ошибок нет.
pub fn sent(stats: &Stats, ctx: &Ctx, channels: &[String], max_block: u64) {
    emit(stats, ctx, SENT, sent_text(channels, max_block));
}

/// Бандл включён в блок. Ошибок нет.
pub fn included(stats: &Stats, ctx: &Ctx, hashes: &[B256], block: Option<u64>) {
    emit(stats, ctx, INCLUDED, included_text(hashes, block));
}

/// Бандл не попал в блок (истёк). Ошибок нет.
pub fn not_included(stats: &Stats, ctx: &Ctx, max_block: u64) {
    emit(stats, ctx, NOT_INCLUDED, not_included_text(max_block));
}
