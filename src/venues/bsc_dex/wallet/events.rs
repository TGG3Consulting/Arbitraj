//! Файл: venues/bsc_dex/wallet/events.rs
//! Что делает: изменения баланса кошелька бота событиями (решение 76) — для зачисления перевода без опроса статуса.
//! Подписка logs «Transfer любого токена на кошелёк / с кошелька» у того же источника, что у пулов
//! (bsc.pool_events_source: WebSocket узла или квитанции блоков — node/source.rs), плюс баланс BNB по блокам, пока
//! ожидается BNB (events_native.rs). Перевод ждёт send.deposit_confirmations блоков (как статус депозита,
//! wallet/transfer_status.rs) и уходит ядру событием MarketEvent::Balance; отменённый реоргом до этого — снимается.
//! Своё (транзакция от адреса кошелька: обмен, свой вывод) помечено own — это не зачисление и не «внешнее изменение».
//! Выключено (venue.balance_events не «да») — подписки нет, ждём изменения настроек.
//! Кто вызывает: resubscribe.rs (задача подписки на рынок).
//! От чего зависит: node/source.rs, events_log.rs, events_tx.rs, events_native.rs, inflows.rs, signer.rs, pools/listen.rs.
//! Настройки: venue.balance_events, send.deposit_confirmations, venue.ws_reconnect_pause_ms [bsc_dex];
//! bsc.pool_events_source (через node/source.rs).

use std::time::Duration;

use serde_json::json;

use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch as Changes;
use crate::storage::journal::Level;

use super::super::node::source::Link;
use super::super::pools::listen::Pools;
use super::events_log::{self, Move};
use super::events_native::Native;
use super::events_tx::Senders;
use super::inflows::Inflows;
use super::signer;

/// Работает постоянно (пока жива подписка на рынок). Ошибок не возвращает: всё — в журнал.
pub async fn run(p: Pools, inflows: Inflows) {
    let names = [
        "venue.balance_events",
        "bsc.pool_events_source",
        "send.deposit_confirmations",
        "dex.native_",
    ];
    let mut changes = Changes::new(
        p.ctx.settings.subscribe(),
        names.iter().map(|n| n.to_string()).collect(),
    );
    let mut told = String::new();
    loop {
        let why = match p.ctx.flag("venue.balance_events") {
            Ok(true) => match signer::address(&p.ctx) {
                Ok(wallet) => {
                    told.clear();
                    tokio::select! {
                        why = session(&p, &inflows, wallet) => why,
                        c = changes.next() => match c {
                            None => return,
                            Some(_) => continue, // настройки изменились — подписка заново сразу
                        },
                    }
                }
                Err(e) => e.message,
            },
            Ok(false) => "venue.balance_events = нет — события кошелька не слушаются".into(),
            Err(e) => e.message,
        };
        if why != told {
            let msg = format!("События баланса кошелька: {why}");
            p.ctx.log(Level::Warn, "dex.balance_event", msg, None).await;
            told = why;
        }
        // Обрыв — переподключение после паузы; выключено или не задано — ждём изменения настроек.
        let pause = p.ctx.int("venue.ws_reconnect_pause_ms").ok();
        let wait = async {
            match pause {
                Some(ms) if p.ctx.flag("venue.balance_events").unwrap_or(false) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await
                }
                _ => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = wait => {}
            c = changes.next() => if c.is_none() { return },
        }
    }
}

/// Одна сессия до обрыва. Принимает: окружение, ожидаемые поступления, кошелёк. Возвращает: причину конца.
async fn session(p: &Pools, inflows: &Inflows, wallet: alloy_primitives::Address) -> String {
    let conn = match Link::open(&p.ctx, &p.feed, &p.heads, &p.out, "wallet").await {
        Ok(c) => c,
        Err(e) => return e.message,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for f in events_log::filters(wallet) {
        match conn.subscribe(json!(["logs", f])).await {
            Ok(mut sub) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    while let Some(raw) = sub.rx.recv().await {
                        let _ = tx.send(raw.get().to_string());
                    }
                });
            }
            Err(e) => return e.message,
        }
    }
    drop(tx);
    let mut heads = p.heads.watch();
    let mut pending: Vec<(Move, bool)> = Vec::new();
    let mut senders = Senders::default();
    let mut native = Native::default();
    loop {
        tokio::select! {
            raw = rx.recv() => {
                let Some(raw) = raw else { break };
                let mv = match serde_json::from_str(&raw).map_err(|e| e.to_string())
                    .and_then(|l| events_log::parse(&l, wallet)) {
                    Ok(Some(mv)) => mv,
                    Ok(None) => continue,
                    Err(why) => {
                        p.ctx.log(Level::Warn, "dex.balance_event", format!("Лог Transfer кошелька не разобран: {why}"), None).await;
                        continue;
                    }
                };
                if mv.removed {
                    pending.retain(|(m, _)| m.key != mv.key);
                    continue;
                }
                // Своя ли транзакция — по отправителю (eth_getTransactionByHash.from). Не узнали — гадать нельзя:
                // ошибка в журнал, лог пропускается (решает опрос статуса).
                match senders.own(&p.ctx, mv.tx, wallet).await {
                    Ok(own) => pending.push((mv, own)),
                    Err(e) => {
                        let msg = format!("Отправитель транзакции {} не прочитан: {} — событие пропущено, решает опрос", mv.tx, e.message);
                        p.ctx.log(Level::Warn, "dex.balance_event", msg, None).await;
                    }
                }
            }
            r = heads.changed() => {
                if r.is_err() { return "головы сети остановлены".into() }
                let Some(head) = heads.borrow_and_update().clone() else { continue };
                let need = match p.ctx.int("send.deposit_confirmations") {
                    Ok(n) => n,
                    Err(e) => return e.message,
                };
                let Some(done) = head.number.checked_sub(need) else { continue };
                let (ready, rest): (Vec<_>, Vec<_>) = pending.drain(..).partition(|(m, _)| m.block <= done);
                pending = rest;
                for (mv, own) in ready {
                    match super::events_tx::change(&p.ctx, &p.tokens, &mv, own).await {
                        Ok(b) => { let _ = p.out.send(MarketEvent::Balance(b)).await; }
                        Err(e) => p.ctx.log(Level::Warn, "dex.balance_event", e.message, None).await,
                    }
                }
                let waiting: Vec<_> = pending.iter().map(|(m, _)| m.tx).collect();
                senders.keep(&waiting);
                match native.step(&p.ctx, inflows, wallet, done).await {
                    Ok(Some(b)) => { let _ = p.out.send(MarketEvent::Balance(b)).await; }
                    Ok(None) => {}
                    Err(e) => p.ctx.log(Level::Warn, "dex.balance_event", format!("Баланс BNB: {}", e.message), None).await,
                }
            }
        }
    }
    conn.end_reason()
        .unwrap_or_else(|| "подписки закрыты".into())
}
