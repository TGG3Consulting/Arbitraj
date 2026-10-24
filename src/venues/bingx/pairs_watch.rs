//! Файл: venues/bingx/pairs_watch.rs
//! Что делает: фоновое обновление справочника пар BingX для текущей подписки: раз в venue.symbols_refresh_s
//! перечитывает «Spot trading symbols» и сравнивает снимок пар монет подписки (pairs.rs) с прежним.
//! Изменился набор стаканов (пара включилась или выключилась по статусу) — пересподписка тем же списком монет
//! (subs.rs → market.rs; она же шлёт ядру SpotPairs). Изменились только флаги API — ядру новый SpotPairs без
//! пересподписки стаканов. Каждое изменение — запись в журнал «ТИКЕР: было → стало».
//! Кто вызывает: subs.rs (запускает при каждой подписке из ядра, останавливает при следующей).
//! От чего зависит: symbols.rs, pairs.rs, subs.rs, ctx.rs, storage/journal.
//! Настройки: venue.symbols_refresh_s (период; читается каждый круг — новое значение действует со следующего).
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
                "Справочник пар BingX не обновлён: {} — действует прежний",
                e.message
            );
            log(&p.ctx, Level::Warn, m, None).await;
            continue;
        }
        let now = Snapshot::build(&p.symbols, &assets);
        if now == last {
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
                }
            }
        } else {
            let _ = out.send(now.event(p.ctx.venue())).await;
            last = now;
        }
    }
}

/// Запись в журнал от имени BingX. Ошибок нет (сбой журнала не останавливает цикл).
async fn log(ctx: &Ctx, level: Level, msg: String, data: Option<serde_json::Value>) {
    let _ = ctx.journal.write(level, CODE, Some(ID), msg, data).await;
}
