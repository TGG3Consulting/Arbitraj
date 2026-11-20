//! Файл: venues/bsc_dex/connection_events.rs
//! Что делает: решение 435 (задача 433-м) — часть «События по WebSocket» в «Проверить подключение» площадки DEX: узел
//! по WebSocket (адрес — bsc.node_ws_url площадки, решение 435 433-ж) принимает подписку eth_subscribe «logs» — ей
//! площадка слушает события пулов. Фильтр — адрес обёртки нативной монеты площадки (dex.native_wrapped): узкий и есть у
//! каждой сети; события не ждутся (их частота от рынка) — проверяется, что узел вернул id подписки. Блоки по WebSocket
//! (newHeads) и сеть узла (eth_chainId) — часть «Узел WebSocket» (connection.rs).
//! Кто вызывает: connection.rs (check).
//! От чего зависит: node/ws.rs (Conn), ctx.rs.
//! Настройки: dex.native_wrapped [площадка DEX]; узел — bsc.node_ws_url (+ резервный, ключи — секреты узла).
//!
//! eth_subscribe logs (фильтр address) — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml ;
//! на своём узле Arbitrum — запись tests/data/arbitrum_node_2026-11-19/07_ws_eth_subscribe_logs.json (подтверждение
//! подписки и события).

use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::node::ws::Conn;

/// Проверка подписки logs. Принимает: окружение. Возвращает: описание (id подписки; dex.native_wrapped не задана — «не
/// проверены» с причиной, как часть «Обёртка нативной монеты») или текст причины (dex.native_wrapped задана не адресом —
/// текст ошибки чтения настройки; нет связи; узел отказал в подписке).
pub async fn check(ctx: &Ctx) -> Result<String, String> {
    let wrapped = match ctx.addr("dex.native_wrapped") {
        Ok(a) => a,
        // Решение 435 (ревью, правки): не задана — проверка не нужна (как раньше); задана неверно — точная причина.
        Err(e) if e.code == ErrorCode::NotConfigured => {
            return Ok(
                "не проверены: не задана dex.native_wrapped (адрес обёртки — фильтр пробной подписки logs)".into(),
            );
        }
        Err(e) => return Err(e.message),
    };
    let run = async {
        let conn = Conn::open(ctx, "check_logs").await?;
        let sub = conn
            .subscribe(json!(["logs", { "address": wrapped }]))
            .await?;
        GatewayResult::Ok(format!(
            "подписка logs принята (id {}, фильтр — dex.native_wrapped {wrapped:#x})",
            sub.id
        ))
    };
    run.await.map_err(|e| e.message)
}
