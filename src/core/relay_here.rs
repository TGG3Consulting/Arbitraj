//! Файл: core/relay_here.rs
//! Что делает: продажа «здесь» в режиме «Перегон» (решение 220, Тигран 08.11.2026; живой случай BRIC: перегон не
//! выгоднее → продано по рынку с убытком −30 %, −11 USDT, без всякого порога). Перед продажей здесь (перегон не
//! выгоднее, перегоны исчерпаны, площадка, где лежит монета, на паузе) убыток deal.loss_pct_if(выручка) сравнивается с
//! stuck.relay_max_loss_pct: не больше — продажа по рынку; больше — алерт человеку с кнопками (как «Порог убытка»),
//! сделка ждёт решения. Настройка не задана — функция не работает: алерт с причиной «не задана настройка» (правило
//! проекта: стартовых значений нет). Нет котировки — алерт с причиной (убыток неизвестен, вслепую не продаём).
//! Котировка — кошельком сделки (trade_ops::deal_value, решение 220 часть А).
//! Ответы Тиграна 08.11.2026 к решению 220: (1) нет котировки — сначала ждём следующей цены монеты (stuck::next_tick:
//! первый же тик — новая котировка, без лишнего ожидания), не дольше stuck.no_quote_wait_ms; не задана или время
//! вышло — алерт, как раньше. (2) Перевод не состоялся (отказ вывода в «Перегоне», «перевод не состоялся» после
//! покупки в executor.rs) — вывод не повторяем: монеты на месте (баланс площадки) — продажа здесь по тем же правилам;
//! монет нет — сделка остановлена, решает человек (transfer_failed).
//! Кто вызывает: core/relay.rs, core/executor.rs (transfer_failed).
//! От чего зависит: trade_ops.rs, stuck.rs (sold_by_market, next_tick), manual.rs (raise_alert), manual_halted.rs
//! (on_hand, set_qty), ledger.rs, deal.rs.
//! Настройки: stuck.relay_max_loss_pct, stuck.no_quote_wait_ms.

use std::future::Future;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::Instant;

use crate::notify::human as h;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::remeasure::Guard;

/// Имя настройки порога убытка продажи здесь в режиме «Перегон».
pub const SETTING: &str = "stuck.relay_max_loss_pct";

/// Имя настройки: сколько ждать цену монеты, когда котировки продажи здесь нет (мс).
pub const WAIT: &str = "stuck.no_quote_wait_ms";

/// Что делать с продажей здесь.
#[derive(Debug, Clone, PartialEq)]
pub enum Here {
    /// Продать по рынку — текст причины (для хронологии и уведомления stuck.market_sold).
    Market(String),
    /// Алерт человеку с кнопками: убыток (None — неизвестен) и причина.
    Alert { loss: Option<Decimal>, why: String },
}

/// Решение по продаже здесь (без сети — чистая функция). Принимает: сделку (для убытка), почему продаём здесь, порог
/// stuck.relay_max_loss_pct (Err — не задан, текст ошибки чтения), выручку при продаже сейчас (Err — нет котировки).
/// Возвращает: Here. Ошибок нет.
pub fn decide(
    deal: &Deal,
    why: &str,
    max: Result<Decimal, String>,
    value: Result<Decimal, String>,
) -> Here {
    let loss = value.as_ref().ok().map(|v| deal.loss_pct_if(*v));
    match (max, loss) {
        // Решение 247, п. Е: подпись по знаку (минус убытка — прибыль), логика порога та же.
        (Ok(max), Some(loss)) if loss <= max => Here::Market(format!(
            "{why}; {} ({SETTING})",
            h::loss_vs_max(loss, max, true)
        )),
        (Ok(max), Some(loss)) => Here::Alert {
            loss: Some(loss),
            why: format!("{why}; {} ({SETTING})", h::loss_vs_max(loss, max, false)),
        },
        (Err(e), loss) => Here::Alert {
            loss,
            why: format!("{why}; продажа здесь не решена: не задана настройка {SETTING} ({e})"),
        },
        (Ok(_), None) => Here::Alert {
            loss: None,
            why: format!(
                "{why}; убыток продажи здесь неизвестен — нет котировки: {}",
                value.err().unwrap_or_default()
            ),
        },
    }
}

/// Продать здесь по рынку, если убыток не выше stuck.relay_max_loss_pct, иначе — алерт человеку. Принимает: ядро,
/// сделку, почему продаём здесь (для хронологии, журнала и Telegram). Ошибок нет: всё — в алерте или продаже.
pub async fn sell_here(core: &Core, deal: Deal, why: &str) {
    let s = &core.inner.deps.settings;
    let max = s.decimal(SETTING, At::default()).map_err(|e| e.message);
    // Порог не задан — алерт и так: цену не ждём (ожидание ничего не решит).
    let wait = max
        .as_ref()
        .ok()
        .and_then(|_| s.integer(WAIT, At::default()).ok());
    // Решение 247: порог продажи здесь — и граница пере-замера продажи DEX (core/remeasure.rs).
    let cap = max.as_ref().ok().copied();
    let d = &deal;
    let value = quote_or_wait(core, d, wait, move || async move {
        super::trade_ops::deal_value(core, d).await.map(|(v, _)| v)
    })
    .await;
    match decide(&deal, why, max, value) {
        Here::Market(text) => {
            let g = cap.map_or(Guard::Off, Guard::MaxLoss);
            super::stuck::sold_by_market(core, deal, &text, g).await
        }
        Here::Alert { loss, why } => super::manual::raise_alert(core, deal, loss, why, None).await,
    }
}

/// Котировка продажи здесь с ожиданием цены (ответ Тиграна 08.11, п. 3). Есть сразу — сразу; нет — ждём тика монеты
/// (stuck::next_tick) и котируем снова на каждом тике, не дольше `wait_ms`. Принимает: ядро, сделку (монета тика),
/// stuck.no_quote_wait_ms (None — не задана: без ожидания), котировку (выручка USDT или текст причины).
/// Возвращает: выручку; Err — котировки нет (текст с причиной и тем, сколько ждали). Ошибок сверх этого нет.
pub async fn quote_or_wait<F, Fut>(
    core: &Core,
    deal: &Deal,
    wait_ms: Option<u64>,
    mut quote: F,
) -> Result<Decimal, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Decimal, String>>,
{
    let mut last = match quote().await {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    let Some(ms) = wait_ms else {
        return Err(format!("{last} ({WAIT} не задана — цену не ждали)"));
    };
    let until = Instant::now() + Duration::from_millis(ms);
    while super::stuck::next_tick(core, deal, Some(until)).await {
        match quote().await {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(format!("{last} (ждали цену монеты {ms} мс, {WAIT})"))
}

/// Что делать после несостоявшегося перевода.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    /// Монеты на месте: продать здесь это количество (заметка о балансе — в хронологию).
    SellHere(Decimal, Option<String>),
    /// Монет на месте нет (или баланс не получен) — остановка с текстом причины.
    Halt(String),
}

/// Решение после несостоявшегося перевода (чистая функция). Принимает: почему (текст отказа перевода), баланс
/// площадки, где сделка (manual_halted::on_hand: количество и заметка; Err — монет нет или баланс не получен).
/// Возвращает: Place. Ошибок нет.
pub fn place(why: &str, on_hand: Result<(Decimal, Option<String>), String>) -> Place {
    match on_hand {
        Ok((q, note)) if q > Decimal::ZERO => Place::SellHere(q, note),
        Ok(_) => Place::Halt(format!("{why}; монет на месте нет — нужна ручная проверка")),
        Err(e) => Place::Halt(format!(
            "{why}; монет на месте нет ({e}) — нужна ручная проверка"
        )),
    }
}

/// Перевод не состоялся (решение 220, ответ Тиграна 08.11, п. 1–2): вывод не повторяем. Монеты на площадке, где
/// сделка, — продажа здесь по порогу (sell_here); нет — сделка остановлена (Step::Halted), решает человек.
/// Принимает: ядро, сделку (исход перевода известен — не OutcomeUnknown), текст отказа. Ошибок нет.
pub async fn transfer_failed(core: &Core, mut deal: Deal, why: &str) {
    let (at, a) = (deal.at.clone(), deal.direction.asset.clone());
    let hand = super::manual_halted::on_hand(core, &deal, &at, &a, deal.qty).await;
    match place(why, hand) {
        Place::SellHere(q, note) => {
            super::manual_halted::set_qty(&mut deal, q, note);
            let text = format!(
                "{}: {why} — {q} {} на {}: вывод не повторяем, продажа здесь по порогу {SETTING}",
                deal.id, a.ticker, at.0
            );
            core.inner
                .deps
                .log(Level::Warn, "core.relay_here", Some(&at.0), &text, None)
                .await;
            sell_here(core, deal, why).await
        }
        Place::Halt(text) => super::ledger::finish(core, &mut deal, Step::Halted, &text).await,
    }
}
