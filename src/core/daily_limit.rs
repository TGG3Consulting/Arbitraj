//! Файл: core/daily_limit.rs
//! Что делает: лимит убытка за день (risk.max_daily_loss_usd; решения 53, 152, 153).
//! Убыток для лимита = −(итог дня − итог дня в момент сброса лимита); без сброса — −итог дня (итог дня = факт закрытых
//! сегодня сделок + оценки позиций, daily.rs). Это не «сумма минусов»: прибыльные сделки дня его уменьшают.
//! Достигнут — система в «Стоп» (Risk.daily_stop = true) до решения человека. Пересчёт убрал превышение (пришёл факт,
//! например «Продать сейчас») — остановка сама НЕ снимается, но плашка и журнал пишут «лимит сейчас не превышен».
//! «Сбросить лимит дня» (решение 153): отметка времени и итога дня в учёте (daily_result, переживает перезапуск, новый
//! день — теряет силу); после неё лимит считает только изменения итога дня после отметки; итог дня, история и сделки
//! не меняются. Журнал core.loss_limit_reset, уведомление risk.daily_limit_reset. Система сама не запускается.
//! Кто вызывает: daily.rs (check — при каждом пересчёте итога дня), risk.rs (view — snapshot и publish), api (reset).
//! От чего зависит: daily.rs (Book, commit, today_book), day_view.rs, risk.rs (Mode, publish), settings.
//! Настройки: risk.max_daily_loss_usd.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::day_view::{DayView, LimitReset, LimitView};
use super::engine::{Core, lk};
use super::risk::Mode;

/// Что делает: состояние лимита по итогу дня и настройке.
/// Принимает: ядро, итог дня. Возвращает: None — лимит не задан. Ошибок нет.
pub fn view(core: &Core, day: &DayView) -> Option<LimitView> {
    let max = core
        .inner
        .deps
        .settings
        .decimal("risk.max_daily_loss_usd", At::default())
        .ok()?;
    let base = day
        .limit_reset
        .as_ref()
        .filter(|r| r.at.date_naive() == day.date)
        .map_or(Decimal::ZERO, |r| r.base_usd);
    let loss = base - day.result_usd;
    Some(LimitView {
        max_usd: max,
        loss_usd: loss.max(Decimal::ZERO),
        exceeded: loss >= max,
    })
}

/// Текст лимита для человека (решение 164): «убыток за день 42.43 USDT больше лимита 15 USDT (из них 89.49 USDT —
/// незакрытые сделки, будут пересчитаны после продажи)», с отметкой сброса, если была. Принимает: итог дня, лимит,
/// связку («больше лимита», «из лимита»). Ошибок нет.
fn describe(day: &DayView, l: &LimitView, rel: &str) -> String {
    use crate::notify::human::{money, usd};
    let reset = day
        .limit_reset
        .as_ref()
        .map(|r| {
            format!(
                "; лимит сброшен в {} UTC (итог дня тогда {})",
                r.at.format("%H:%M"),
                usd(r.base_usd)
            )
        })
        .unwrap_or_default();
    let prov = -day.provisional_usd;
    let prov = if prov.is_zero() {
        String::new()
    } else {
        format!(
            " (из них {} — незакрытые сделки, будут пересчитаны после продажи)",
            money(prov)
        )
    };
    format!(
        "убыток за день {} {rel} {}{prov}{reset}",
        usd(l.loss_usd),
        usd(l.max_usd)
    )
}

/// Что делает: проверка лимита после пересчёта итога дня; достигнут — «Стоп».
/// Возвращает: (Some(текст) — остановлена сейчас; Some(текст) — остановлена по лимиту, а он больше не превышен).
pub fn check(core: &Core) -> (Option<String>, Option<String>) {
    let day = lk(&core.inner.risk).day.clone();
    let l = view(core, &day);
    let out = {
        let mut r = lk(&core.inner.risk);
        let was = r.day.limit.as_ref().is_some_and(|x| x.exceeded);
        r.day.limit = l.clone();
        match &l {
            Some(l) if l.exceeded && r.mode != Mode::Stopped => {
                let t = describe(&day, l, "больше лимита");
                r.mode = Mode::Stopped;
                r.reason = Some(t.clone());
                r.daily_stop = true;
                (Some(t), None)
            }
            Some(l) if !l.exceeded && was && r.daily_stop => (
                None,
                Some(format!(
                    "Лимит дня сейчас не превышен: {}. Остановка по лимиту сама не снимается — запуск кнопкой «Старт»",
                    describe(&day, l, "из лимита")
                )),
            ),
            _ => (None, None),
        }
    };
    super::risk::publish(core);
    out
}

/// Что делает: «Сбросить лимит дня» (решение 153): отметка времени и итога дня; журнал, уведомление.
/// Принимает: ядро, кто нажал. Возвращает: текст для журнала. Ошибок нет (ошибка базы — в журнал из commit).
pub async fn reset(core: &Core, who: &str) -> String {
    let (today, mut book) = super::daily::today_book(core);
    let base = book.final_usd + super::daily::prov_on(&book, today);
    let at = Utc::now();
    book.limit_reset = Some(LimitReset {
        at,
        base_usd: base,
        by: who.to_string(),
    });
    super::daily::commit(core, today, book).await;
    let stopped = lk(&core.inner.risk).daily_stop;
    let text = format!(
        "Лимит дня сброшен ({who}) в {} UTC: итог дня в этот момент {} USDT (убыток для лимита до сброса {}). Дальше \
         лимит считает только изменения итога дня после отметки; итог дня, история и сделки не меняются.{}",
        at.format("%H:%M:%S"),
        base.round_dp(4),
        (-base).max(Decimal::ZERO).round_dp(4),
        if stopped {
            " Система остаётся в «Стоп» (остановлена по лимиту дня) — запуск кнопкой «Старт»"
        } else {
            ""
        }
    );
    // Решение 164: в Telegram — коротко; подробности — в журнале.
    let tg = crate::notify::human::Msg::new(crate::notify::human::INFO, "Лимит дня сброшен")
        .kv("Кто", who)
        .line(format!(
            "Дальше лимит считает только убытки после {} UTC; итог дня и сделки не меняются",
            at.format("%H:%M")
        ))
        .line(if stopped {
            "Торговля остаётся остановленной"
        } else {
            ""
        })
        .render();
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.loss_limit_reset", None, &text, None)
        .await;
    deps.notify("risk.daily_limit_reset", None, &tg).await;
    text
}

impl Core {
    /// Кнопка «Сбросить лимит дня» (интерфейс, решение 153). Возвращает: текст для журнала. Ошибок нет.
    pub async fn reset_daily_limit(&self, who: &str) -> String {
        reset(self, who).await
    }
}
