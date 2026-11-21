//! Файл: venues/bybit/private_cache.rs
//! Что делает: ордера и исполнения из приватного потока Bybit (order.spot, execution.spot) в памяти — по orderId.
//! Ожидание итога ордера (order_state.rs) сначала смотрит сюда и просыпается по событию (Notify), а запрос REST — только
//! если события нет («This request is asynchronous so please use the websocket to confirm the order status», Place
//! Order). Записи ордера убираются после итога (forget), чтобы память не росла.
//! Кто вызывает: user_stream_conn.rs (кладёт), order_state.rs и fills.rs (читают), ctx.rs (хранит).
//! От чего зависит: serde_json, tokio (Notify), num.rs.
//! Настройки: нет.
//!
//! Официальная документация: Order https://bybit-exchange.github.io/docs/v5/websocket/private/order (data[]: orderId,
//! orderStatus, cumExecQty, cumExecValue, avgPrice, cumFeeDetail, updatedTime; «You may receive two orderStatus=Filled
//! messages» — берётся последнее по updatedTime), Execution https://bybit-exchange.github.io/docs/v5/websocket/private/execution
//! (data[]: orderId, execId, execQty, execFee, feeCurrency).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use tokio::sync::Notify;

use super::num;

/// Состояние в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Private {
    orders: Arc<RwLock<HashMap<String, Value>>>,
    /// orderId → исполнения (execId → запись).
    fills: Arc<RwLock<HashMap<String, HashMap<String, Value>>>>,
    wake: Arc<Notify>,
}

impl Private {
    /// Кладёт запись ордера (более старую по updatedTime не ставит поверх новой). Ошибок нет.
    pub fn put_order(&self, o: &Value) {
        let Some(id) = num::text_opt(o, "orderId") else {
            return;
        };
        let t = |v: &Value| num::text_opt(v, "updatedTime").and_then(|s| s.parse::<i64>().ok());
        let mut m = self.orders.write().unwrap_or_else(|p| p.into_inner());
        if m.get(&id).is_some_and(|old| t(old) > t(o)) {
            return;
        }
        m.insert(id, o.clone());
        drop(m);
        self.wake.notify_waiters();
    }

    /// Кладёт исполнение (без повторов по execId). Ошибок нет.
    pub fn put_fill(&self, f: &Value) {
        let (Some(id), Some(exec)) = (num::text_opt(f, "orderId"), num::text_opt(f, "execId"))
        else {
            return;
        };
        self.fills
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id)
            .or_default()
            .insert(exec, f.clone());
        self.wake.notify_waiters();
    }

    /// Последняя запись ордера из потока. None — событий не было. Ошибок нет.
    pub fn order(&self, id: &str) -> Option<Value> {
        self.orders
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    /// Исполнения ордера из потока. Ошибок нет.
    pub fn fills(&self, id: &str) -> Vec<Value> {
        self.fills
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Ждёт следующего события потока не дольше gap. Ошибок нет.
    pub async fn wait(&self, gap: std::time::Duration) {
        let _ = tokio::time::timeout(gap, self.wake.notified()).await;
    }

    /// Убирает записи ордера (итог получен). Ошибок нет.
    pub fn forget(&self, id: &str) {
        self.orders
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.fills
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
    }
}
