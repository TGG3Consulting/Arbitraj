//! Файл: venues/bsc_dex/pools/refresh_urgency.rs
//! Что делает: решение 314 — класс срочности очереди узла BSC (gateway/urgency.rs: trade → check → quote → background)
//! для перечитываний пула, вызванных событием пула в сессии пачки. У сессии пачки класс не задан — её обращения к узлу
//! идут классом Background (низший; доля фона bsc.node_background_share_pct) после Trade/Check/Quote. Задана
//! dex.event_refresh_urgency — перечитывание по событию идёт заданным классом. Класс ставится ТОЛЬКО на время одного
//! перечитывания (`urgent` — gateway/urgency.rs scope_opt вокруг future перечитывания): task_local действует, пока
//! опрашивается эта future, и снимается при её завершении — остальная работа сессии (приём пулов, пробуждение мелких,
//! повтор, сверка настроек) остаётся Background; задачи, запущенные изнутри через spawn, класс не наследуют.
//! Не задана (или значение вне списка) — класс не ставится, всё как раньше.
//! Покрыто: listen_log.rs — off_map и Refresh (через refresh_event.rs, вместе с eth_blockNumber реорга), Bins (бины
//! на блоке события); listen_site.rs — одна перечитка пар многомонетного пула (и eth_blockNumber реорга);
//! session.rs — пары общего контракта WooFi по логу (per_block::on_member_log). Не покрыто: пробуждение мелкого пула
//! по событию ликвидности (admit::wake — пул не глубокий, в котировки не идёт), повтор чтения на новых блоках
//! (reload.rs), чтения каждого блока (per_block::on_head).
//! Кто вызывает: pools/refresh_event.rs, pools/listen_log.rs, pools/listen_site.rs, pools/session.rs.
//! От чего зависит: ctx.rs (настройка), gateway/urgency.rs.
//! Настройки: dex.event_refresh_urgency [bsc_dex] (trade / check / quote / background; необязательная).

use std::future::Future;

use crate::gateway::urgency::{self, Urgency};

use super::super::ctx::Ctx;

/// Класс из dex.event_refresh_urgency. None — не задана или значение не из списка (класс не ставится). Ошибок нет.
pub fn class(ctx: &Ctx) -> Option<Urgency> {
    parse(&ctx.text("dex.event_refresh_urgency").ok()?)
}

/// Значение настройки → класс: trade, check, quote, background (имена классов gateway/urgency.rs). Иное — None.
/// Ошибок нет.
pub fn parse(v: &str) -> Option<Urgency> {
    match v {
        "trade" => Some(Urgency::Trade),
        "check" => Some(Urgency::Check),
        "quote" => Some(Urgency::Quote),
        "background" => Some(Urgency::Background),
        _ => None,
    }
}

/// Выполняет одно перечитывание по событию с классом `class` (None — без класса, как раньше). Класс действует только
/// внутри `work`. Принимает: класс, работу. Возвращает: её результат. Ошибок нет.
pub async fn with<F: Future>(class: Option<Urgency>, work: F) -> F::Output {
    urgency::scope_opt(class, work).await
}

/// То же с классом из настройки (читается один раз на вызов). Принимает: окружение, работу. Ошибок нет.
pub async fn urgent<F: Future>(ctx: &Ctx, work: F) -> F::Output {
    with(class(ctx), work).await
}
