//! Файл: core/signal_hold_log.rs
//! Что делает: решения 317 и 318 — запись журнала core.signal_hold, когда направление СТАЛО кандидатом (раньше было
//! не кандидатом): когда спред впервые стал выше порога (above_since), сколько мс он держался и сколько обновлений,
//! что запустило кандидата (тик биржи / событие пула / таймер дозревания / пропуск удержания по событию пула),
//! сколько мс стояла цена стороны биржи. Нужна, чтобы на живом боте доказать выигрыш по времени (сравнить с
//! core.window «Окно открыто» и core.candidate_rejected «финальная котировка»).
//! Запись идёт своей задачей — пересчёт монеты журнал не ждёт (как решение 129).
//! Кто вызывает: core/evaluate.rs.
//! От чего зависит: core/engine.rs (Deps.log), core/signal.rs (SpreadView), core/signal_fast.rs (Trigger), settings.
//! Настройки: log.signal_hold (необязательна; не задана или нет — записи нет, одно чтение настройки на переход).

use serde_json::json;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::signal::SpreadView;
use super::signal_fast::Trigger;

/// Что делает: подпись того, что запустило кандидата.
/// Принимает: строку «Живого экрана». Возвращает: текст. Ошибок нет.
pub fn why(v: &SpreadView) -> &'static str {
    if v.skipped_hold {
        return "пропуск удержания: событие пула, цена биржи стоит (решение 317)";
    }
    match v.trigger {
        Trigger::Book => "тик биржи",
        Trigger::Pool => "событие пула",
        Trigger::Timer => "таймер дозревания (решение 318)",
    }
}

/// Что делает: пишет core.signal_hold, если включено log.signal_hold.
/// Принимает: ядро, строку «Живого экрана» нового кандидата. Ошибок нет (журнал — своей задачей).
pub fn note(core: &Core, v: &SpreadView) {
    let deps = &core.inner.deps;
    if !deps
        .settings
        .flag("log.signal_hold", At::default())
        .unwrap_or(false)
    {
        return;
    }
    let held_ms = v.above_since.map(|s| (v.at - s).num_milliseconds().max(0));
    let d = &v.direction;
    let text = format!(
        "Кандидат {} ({} → {}): запуск — {}; спред выше порога с {} ({} мс), обновлений {}; цена биржи стоит {} мс",
        d.asset.ticker,
        d.buy_on.0,
        d.sell_on.0,
        why(v),
        v.above_since
            .map(|s| s.format("%H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| "—".into()),
        held_ms.map(|m| m.to_string()).unwrap_or_else(|| "—".into()),
        v.updates,
        v.cex_still_ms
            .map(|m| m.to_string())
            .unwrap_or_else(|| "—".into()),
    );
    let data = json!({
        "asset": d.asset.ticker,
        "buy_on": d.buy_on.0,
        "sell_on": d.sell_on.0,
        "trigger": v.trigger,
        "skipped_hold": v.skipped_hold,
        "above_since": v.above_since,
        "held_ms": held_ms,
        "updates": v.updates,
        "cex_still_ms": v.cex_still_ms,
        "spread_pct": v.spread_pct,
        "at": v.at,
    });
    let venue = d.buy_on.0.clone();
    let deps = deps.clone();
    tokio::spawn(async move {
        deps.log(
            Level::Info,
            "core.signal_hold",
            Some(&venue),
            text,
            Some(data),
        )
        .await;
    });
}
