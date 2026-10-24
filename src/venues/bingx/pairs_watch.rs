//! Файл: venues/bingx/pairs_watch.rs
//! Что делает: фоновое обновление справочника пар BingX для текущей подписки: раз в venue.symbols_refresh_s
//! перечитывает «Spot trading symbols» и сравнивает снимок пар монет подписки (pairs.rs) с прежним.
//! Изменился набор стаканов (пара включилась или выключилась по статусу) — пересподписка тем же списком монет
//! (subs.rs → market.rs; она же шлёт ядру SpotPairs). Изменились только флаги API — ядру новый SpotPairs без
//! пересподписки стаканов. Каждое изменение — запись в журнал «ТИКЕР: было → стало». После каждой попытки (и при сбое)
//! ядру уходит SpotPairs со временем последнего успешного получения: обновление не проходит дольше
//! venue.symbols_stale_s — ядро отсеивает пары этой биржи (core/pairs_fresh.rs) и не торгует по старым флагам.
//! Кто вызывает: subs.rs (запускает при каждой подписке из ядра, останавливает при следующей).
//! От чего зависит: symbols.rs, pairs.rs, subs.rs, ctx.rs, storage/journal.
//! Настройки: venue.symbols_refresh_s (период; читается каждый круг — новое значение действует со следующего);
//! порог устаревания venue.symbols_stale_s читает ядро.
//!
//! Официальная документация: «Spot trading symbols» GET /openApi/spot/v1/common/symbols
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Market%20Data/Spot%20trading%20symbols (лимит 1 запрос/с с IP —
//! api-ai-skills/spot-market; период задаёт Тигран).

use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::pairs::Snapshot;
use super::subs::{Parts, Subs};

/// Код записей журнала.
const CODE: &str = "venue.symbols";

/// Цикл обновления. Принимает: подписку, части адаптера, монеты подписки, канал событий, снимок пар подписки.
/// Работает, пока задачу не остановят (новая подписка). Не задан venue.symbols_refresh_s — запись в журнал и выход
/// (подписка без него и не начнётся: market.rs → symbols.ensure_fresh). Ошибки запроса — в журнал, прежний справочник
/// остаётся, повтор через период.
pub async fn run(
    subs: Subs,
    p: Parts,
    assets: Vec<Asset>,
    out: mpsc::Sender<MarketEvent>,
    mut last: Snapshot,
) {
    loop {
        let period = match p.ctx.int("venue.symbols_refresh_s") {
            Ok(v) => v,
            Err(e) => {
                let m = format!("Справочник пар BingX не обновляется: {}", e.message);
                log(&p.ctx, Level::Error, m, None).await;
                return;
            }
        };
        tokio::time::sleep(Duration::from_secs(period)).await;
        if let Err(e) = p.symbols.refresh(&p.ctx).await {
            let m = format!(
                "Справочник пар BingX не обновлён: {} — действует прежний; дольше venue.symbols_stale_s — ядро не торгует по нему",
                e.message
            );
            log(&p.ctx, Level::Warn, m, None).await;
            // Прежний снимок с прежним временем получения: ядро видит возраст справочника (core/pairs_fresh.rs).
            send(&p, &out, &last).await;
            continue;
        }
        let now = Snapshot::build(&p.symbols, &assets);
        if now == last {
            send(&p, &out, &now).await;
            continue;
        }
        let changes = now.changes(&last);
        let books_changed = now.books != last.books;
        let m = format!(
            "Справочник пар BingX изменился у {} монет{}",
            changes.len(),
            if books_changed {
                " — пересподписка стаканов"
            } else {
                " — только покупка/продажа через API"
            }
        );
        log(&p.ctx, Level::Warn, m, Some(json!({ "changes": changes }))).await;
        if books_changed {
            match subs.resubscribe(&p, assets.clone(), out.clone()).await {
                Ok(s) => last = s,
                Err(e) => {
                    let m = format!(
                        "Пересподписка BingX не удалась: {} — повтор через venue.symbols_refresh_s",
                        e.message
                    );
                    log(&p.ctx, Level::Warn, m, None).await;
                    send(&p, &out, &now).await;
                }
            }
        } else {
            send(&p, &out, &now).await;
            last = now;
        }
    }
}

/// Шлёт ядру SpotPairs по снимку со временем последнего успешного получения справочника. Ошибок нет
/// (справочник ни разу не получен — не шлёт: подписки без него не бывает, market.rs).
async fn send(p: &Parts, out: &mpsc::Sender<MarketEvent>, snap: &Snapshot) {
    if let Some(at) = p.symbols.loaded_at() {
        let _ = out.send(snap.event(p.ctx.venue(), at)).await;
    }
}

/// Запись в журнал от имени BingX. Ошибок нет (сбой журнала не останавливает цикл).
async fn log(ctx: &Ctx, level: Level, msg: String, data: Option<serde_json::Value>) {
    let _ = ctx.journal.write(level, CODE, Some(ID), msg, data).await;
}
