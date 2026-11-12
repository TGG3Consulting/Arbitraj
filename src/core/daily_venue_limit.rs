//! Файл: core/daily_venue_limit.rs
//! Что делает: лимит убытка за день по паре «площадка × тип направления» (решение 258, Тигран 12.11.2026: «Каждое
//! направление имеет свой лимит. Все.»; было — по площадке, решение 173). Лимит — risk.max_daily_loss_usd: значение
//! пары → значение площадки → вся система (day_pair::max). Убыток пары = −(её итог дня − её итог в момент сброса); без
//! сброса — −итог (итог — daily_venue.rs). Достигнут — ПАУЗА ТОЛЬКО этого направления этой биржи (Risk.pair_paused):
//! новые сделки этой пары не начинаются (risk.rs can_open), идущие доводятся; другие направления этой биржи и все
//! остальные торгуют. Пример: исчерпан «DEX → Binance» — встаёт только DEX → Binance; Binance → DEX и BingX → Binance
//! (это «биржа → биржа» BingX) торгуют. Причина паузы: «DEX → MEXC: убыток за день X больше лимита Y». «DEX ↔ DEX» —
//! пауза только циклов (биржи ↔ DEX торгуют). Пауза пары — отдельно от пауз площадки (Risk.venue_paused: ошибки
//! подряд, правило, исход неизвестен, лимит вывода, сверка, человек) и их не трогает; прочие паузы площадки блокируют
//! вход как раньше.
//! Повторная пауза: после «Снять паузу» без сброса пара торгует, пока её убыток не вырастет — новый минус при
//! превышенном лимите снова ставит паузу; плюсовая сделка или чужая пара паузу не возвращают.
//! Снятие: «Снять паузу» пары или площадки (risk.rs), «Сбросить лимит дня» пары, площадки (все её пары) или общий
//! (daily_limit.rs); решение 225 — сама с новым днём UTC (day_roll.rs).
//! Кто вызывает: daily_commit.rs (check), daily_limit.rs (общий сброс), risk.rs (snapshot), commands_control.rs, api.
//! От чего зависит: daily.rs (today_book, commit), daily_venue.rs, day_pair.rs, daily_limit.rs (describe),
//! daily_commit.rs (announce), day_view.rs.
//! Настройки: risk.max_daily_loss_usd (пара → площадка → вся система).

use chrono::{NaiveDate, Utc};

use crate::notify::human::{INFO, Msg, venue as name};
use crate::storage::journal::Level;

use super::day_pair::{is_pair, label};
use super::day_view::{LimitReset, LimitView, PairDay};
use super::engine::{Core, lk};

/// Что делает: лимит пары по её итогу дня и настройке. Принимает: ядро, id пары, итог пары, день.
/// Возвращает: None — risk.max_daily_loss_usd не задана ни для пары, ни для площадки, ни для всей системы.
pub fn view(core: &Core, pair: &str, x: &PairDay, date: NaiveDate) -> Option<LimitView> {
    let max = super::day_pair::max(core, pair).ok()?;
    Some(LimitView::of(
        max,
        x.limit_reset.as_ref(),
        date,
        x.result_usd,
    ))
}

/// Текст лимита: «DEX → MEXC: убыток за день 20 USDT больше лимита 15 USDT (…)». Ошибок нет.
fn describe(pair: &str, x: &PairDay, l: &LimitView, rel: &str) -> String {
    let t = super::daily_limit::describe(x.limit_reset.as_ref(), x.provisional_usd, l, rel);
    format!("{}: {t}", label(pair))
}

/// Что делает: лимиты пар после пересчёта итога дня; достигнут — пауза пары.
/// Возвращает: (пары, поставленные на паузу сейчас, с причиной; тексты «лимит пары сейчас не превышен»).
pub(super) fn check(core: &Core) -> (Vec<(String, String)>, Vec<String>) {
    let mut paused = Vec::new();
    let mut cleared = Vec::new();
    let mut r = lk(&core.inner.risk);
    let date = r.day.date;
    let names: Vec<String> = r.day.pairs.keys().cloned().collect();
    for p in names {
        let x = r.day.pairs[&p].clone();
        let l = view(core, &p, &x, date);
        let on = r.pair_paused.contains_key(&p);
        let seen = r.loss_seen.get(&p).copied();
        match &l {
            Some(l) => {
                r.loss_seen.insert(p.clone(), l.loss_usd);
                // Пауза — при переходе через лимит или при росте убытка (после «Снять паузу» без сброса).
                let grew = seen.is_none_or(|s| s < l.max_usd || l.loss_usd > s);
                if l.exceeded && grew && !on {
                    let t = describe(&p, &x, l, "больше лимита");
                    r.pair_paused.insert(p.clone(), t.clone());
                    paused.push((p.clone(), t));
                } else if !l.exceeded && on && seen.is_some_and(|s| s >= l.max_usd) {
                    let t = super::daily_limit::describe(
                        x.limit_reset.as_ref(),
                        x.provisional_usd,
                        l,
                        "из лимита",
                    );
                    cleared.push(format!(
                        "{}: лимит дня сейчас не превышен: {t}. Пауза направления снимется сама с новым днём (UTC) — \
                         или «Снять паузу» / «Сбросить лимит дня» по направлению",
                        label(&p)
                    ));
                }
            }
            None => {
                r.loss_seen.remove(&p);
            }
        }
        let paused_now = r.pair_paused.contains_key(&p);
        if let Some(e) = r.day.pairs.get_mut(&p) {
            e.limit = l;
            e.paused = paused_now;
        }
    }
    (paused, cleared)
}

/// Что делает: «Сбросить лимит дня» по паре или по площадке (все её пары, day_pair::of_venue): отметка времени и итога
/// каждой пары; их паузы по убытку дня снимаются; журнал core.loss_limit_reset, уведомление risk.daily_limit_reset.
/// Итог дня, история и сделки не меняются. Принимает: ядро, id пары или площадки, кто нажал. Возвращает: текст для
/// журнала. Ошибок нет.
pub async fn reset(core: &Core, id: &str, who: &str) -> String {
    let pairs = if is_pair(id) {
        vec![id.to_string()]
    } else {
        super::day_pair::of_venue(core, id)
    };
    let (today, mut book) = super::daily::today_book(core);
    let days = super::daily_venue::days(core, &book, today);
    let at = Utc::now();
    let (mut parts, mut lifted) = (Vec::new(), Vec::new());
    for p in &pairs {
        let x = days.get(p).cloned().unwrap_or_default();
        book.pair_resets.insert(
            p.clone(),
            LimitReset {
                at,
                base_usd: x.result_usd,
                by: who.to_string(),
            },
        );
        parts.push(format!("{} {} USDT", label(p), x.result_usd.round_dp(4)));
        if lift(core, p) {
            lifted.push(label(p));
        }
    }
    let trip = super::daily::commit(core, today, book).await;
    let what = if is_pair(id) {
        format!("направления {}", label(id))
    } else {
        format!("площадки {} (все её направления)", name(id))
    };
    let lifted = lifted.join(", ");
    let text = format!(
        "Лимит дня {what} сброшен ({who}) в {} UTC: итог дня в этот момент — {}. Дальше лимит считает только изменения \
         после отметки; итог дня, история и сделки не меняются.{}",
        at.format("%H:%M:%S"),
        parts.join("; "),
        if lifted.is_empty() {
            String::new()
        } else {
            format!(" Пауза по убытку дня снята: {lifted} — снова торгует")
        }
    );
    let tg = Msg::new(INFO, format!("Лимит дня {what} сброшен"))
        .kv("Кто", who)
        .line(format!(
            "Дальше лимит считает только убытки после {} UTC; итог дня и сделки не меняются",
            at.format("%H:%M")
        ))
        .line(if lifted.is_empty() {
            String::new()
        } else {
            format!("Пауза снята: {lifted} — торговля идёт")
        })
        .render();
    let v = if is_pair(id) {
        super::day_pair::venue_or_dex(core, id).unwrap_or_default()
    } else {
        id.to_string()
    };
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.loss_limit_reset", Some(&v), &text, None)
        .await;
    deps.notify("risk.daily_limit_reset", Some(&v), &tg).await;
    super::daily_commit::announce(core, &trip).await;
    text
}

/// Снимает паузу пары по убытку дня и забывает замеченный убыток. Принимает: ядро, id пары. Возвращает: была ли
/// снята пауза. Ошибок нет.
pub(super) fn lift(core: &Core, pair: &str) -> bool {
    let mut r = lk(&core.inner.risk);
    r.loss_seen.remove(pair);
    r.pair_paused.remove(pair).is_some()
}

impl Core {
    /// «Сбросить лимит дня» пары или площадки (интерфейс, Telegram; решения 173, 258). Принимает: id пары
    /// («bingx:dex_cex», «dex_dex») или площадки (все её пары), кто. Возвращает: текст для журнала.
    pub async fn reset_venue_daily_limit(&self, id: &str, who: &str) -> String {
        reset(self, id, who).await
    }
}
