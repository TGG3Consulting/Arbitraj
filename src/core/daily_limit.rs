//! Файл: core/daily_limit.rs
//! Что делает: общий лимит убытка за день всей системы (решения 53, 152, 153, 173) и общий «Сбросить лимит дня».
//! Решение 173: обычный дневной лимит — по площадке (risk.max_daily_loss_usd, daily_venue_limit.rs: пауза только
//! площадки-нарушителя). Полный «Стоп» всей системы по убытку — только отдельной настройкой
//! risk.max_daily_loss_total_usd (вся система; не задана — общего стопа по убытку нет).
//! Убыток для общего лимита = −(итог дня − итог дня в момент общего сброса); без сброса — −итог дня (итог дня = факт
//! закрытых сегодня сделок + оценки позиций, daily.rs). Это не «сумма минусов»: прибыльные сделки дня его уменьшают.
//! Достигнут — система в «Стоп» (Risk.daily_stop = true) до решения человека. Пересчёт убрал превышение — остановка
//! сама НЕ снимается, но плашка и журнал пишут «лимит сейчас не превышен».
//! «Сбросить лимит дня» общий (решение 153): отметка времени и итога дня — всей системы и каждой площадки; паузы
//! площадок по убытку дня снимаются; система из «Стоп» сама не запускается. По одной площадке — daily_venue_limit.rs.
//! Отметки — в учёте (daily_result, переживают перезапуск, новый день — теряют силу). Журнал core.loss_limit_reset,
//! уведомление risk.daily_limit_reset.
//! Кто вызывает: daily_commit.rs (check), risk.rs (view — snapshot), daily_venue_limit.rs (describe), api (reset).
//! От чего зависит: daily.rs (today_book, commit), daily_venue.rs, daily_venue_limit.rs, day_view.rs, risk.rs, settings.
//! Настройки: risk.max_daily_loss_total_usd.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::notify::human::{INFO, Msg, money, usd};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::day_view::{DayView, LimitReset, LimitView};
use super::engine::{Core, lk};
use super::risk::Mode;

/// Что делает: общий лимит по итогу дня и настройке. Возвращает: None — risk.max_daily_loss_total_usd не задана.
pub fn view(core: &Core, day: &DayView) -> Option<LimitView> {
    let s = &core.inner.deps.settings;
    let max = s
        .decimal("risk.max_daily_loss_total_usd", At::default())
        .ok()?;
    Some(LimitView::of(
        max,
        day.limit_reset.as_ref(),
        day.date,
        day.result_usd,
    ))
}

/// Текст лимита для человека (решение 164): «убыток за день 42.43 USDT больше лимита 15 USDT (из них 89.49 USDT —
/// незакрытые сделки, будут пересчитаны после продажи)», с отметкой сброса, если была. Принимает: отметку сброса,
/// оценку позиций дня, лимит, связку («больше лимита», «из лимита»). Ошибок нет.
pub(super) fn describe(
    reset: Option<&LimitReset>,
    provisional: Decimal,
    l: &LimitView,
    rel: &str,
) -> String {
    let reset = reset
        .map(|r| {
            format!(
                "; лимит сброшен в {} UTC (итог тогда {})",
                r.at.format("%H:%M"),
                usd(r.base_usd)
            )
        })
        .unwrap_or_default();
    let prov = -provisional;
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

/// Что делает: проверка общего лимита после пересчёта итога дня; достигнут — «Стоп».
/// Возвращает: (Some(текст) — остановлена сейчас; Some(текст) — остановлена по лимиту, а он больше не превышен).
pub fn check(core: &Core) -> (Option<String>, Option<String>) {
    let day = lk(&core.inner.risk).day.clone();
    let l = view(core, &day);
    let text = |l: &LimitView, rel: &str| {
        let t = describe(day.limit_reset.as_ref(), day.provisional_usd, l, rel);
        format!("вся система: {t}")
    };
    let out = {
        let mut r = lk(&core.inner.risk);
        let was = r.day.limit.as_ref().is_some_and(|x| x.exceeded);
        r.day.limit = l.clone();
        match &l {
            Some(l) if l.exceeded && r.mode != Mode::Stopped => {
                let t = text(l, "больше общего лимита");
                r.mode = Mode::Stopped;
                r.reason = Some(t.clone());
                r.daily_stop = true;
                (Some(t), None)
            }
            Some(l) if !l.exceeded && was && r.daily_stop => (
                None,
                Some(format!(
                    "Лимит дня сейчас не превышен: {}. Остановка по лимиту сама не снимается — запуск кнопкой «Старт»",
                    text(l, "из общего лимита")
                )),
            ),
            _ => (None, None),
        }
    };
    super::risk::publish(core);
    out
}

/// Что делает: общий «Сбросить лимит дня»: отметка времени и итога дня — всей системы и каждой площадки, паузы
/// площадок по убытку дня снимаются; журнал, уведомление. Принимает: ядро, кто нажал. Возвращает: текст для журнала.
pub async fn reset(core: &Core, who: &str) -> String {
    let (today, mut book) = super::daily::today_book(core);
    let base = book.final_usd + super::daily::prov_on(&book, today);
    let at = Utc::now();
    let mark = |base_usd| LimitReset {
        at,
        base_usd,
        by: who.to_string(),
    };
    book.limit_reset = Some(mark(base));
    let mut lifted = Vec::new();
    for (v, x) in super::daily_venue::days(core, &book, today) {
        book.venue_resets.insert(v.clone(), mark(x.result_usd));
        if super::daily_venue_limit::lift(core, &v) {
            lifted.push(crate::notify::human::venue(&v));
        }
    }
    let trip = super::daily::commit(core, today, book).await;
    let stopped = lk(&core.inner.risk).daily_stop;
    let lifted = lifted.join(", ");
    let text = format!(
        "Лимит дня сброшен ({who}) в {} UTC — вся система и все площадки: итог дня в этот момент {} USDT (убыток для \
         лимита до сброса {}). Дальше лимиты считают только изменения итога дня после отметки; итог дня, история и \
         сделки не меняются.{}{}",
        at.format("%H:%M:%S"),
        base.round_dp(4),
        (-base).max(Decimal::ZERO).round_dp(4),
        if lifted.is_empty() {
            String::new()
        } else {
            format!(" Паузы площадок по убытку дня сняты: {lifted}.")
        },
        if stopped {
            " Система остаётся в «Стоп» (остановлена по общему лимиту дня) — запуск кнопкой «Старт»"
        } else {
            ""
        }
    );
    // Решение 164: в Telegram — коротко; подробности — в журнале.
    let tg = Msg::new(INFO, "Лимит дня сброшен")
        .kv("Кто", who)
        .line(format!(
            "Вся система и все площадки: дальше лимиты считают только убытки после {} UTC; итог дня и сделки не меняются",
            at.format("%H:%M")
        ))
        .line(if lifted.is_empty() {
            String::new()
        } else {
            format!("Снята пауза: {lifted}")
        })
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
    super::daily_commit::announce(core, &trip).await;
    text
}

impl Core {
    /// Общий «Сбросить лимит дня» (интерфейс, решения 153, 173). Возвращает: текст для журнала. Ошибок нет.
    pub async fn reset_daily_limit(&self, who: &str) -> String {
        reset(self, who).await
    }
}
