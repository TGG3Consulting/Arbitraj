//! Файл: core/drawdown_snap.rs
//! Что делает: решение 414 — снятие СЛЕПКА (опорного баланса) направления для лимита просадки от пика:
//! - когда: новый день UTC (day_roll.rs), «Сбросить лимит дня» по направлению / площадке / общий
//!   (daily_venue_limit.rs, daily_limit.rs), запуск (нет слепка этого дня или запрос не досняли до перезапуска), задали
//!   risk.drawdown_* (направление перешло на просадку — слепка ещё нет). Только у пар с заданными risk.drawdown_*
//!   (drawdown_cfg.rs); пары по старому лимиту дня слепков не снимают;
//! - как: запрос (Want) пишется в книгу дня (переживает перезапуск), задача пары ждёт до CLEAN_WAIT (2 минуты,
//!   константа — решение Тиграна 19.11) момента, когда у направления НЕТ идущих сделок (drawdown_ledger.rs active —
//!   набор unsettled.rs: не закрыта, возврат USDT не закончен, выручка ждёт общего возврата группы докупки), будится
//!   каждым сохранением сделки (живой поток ядра). Дождалась — слепок «чистый»: USDT на площадке покупки по факту
//!   (drawdown_fact.rs). Не дождалась — «по учёту»: факт + деньги идущих сделок этого направления, ещё не вернувшиеся на
//!   площадку покупки (drawdown_ledger.rs own). В обоих случаях вычитается чужое — USDT сделок других направлений на
//!   этом счёте (выручка, ждущая возврата на их площадку покупки): иначе слепок BingX → DEX включал бы выручку
//!   DEX → BingX, ждущую перегона в кошелёк (уточнение агента t199 к п. 2: в чистом случае своё = 0, чужое — то же);
//! - пока слепок не снят (ждём, баланс не читается) — действуют прежний пик и счётчик; баланс не прочитался — запись в
//!   журнал, причина на экране (Want.note), повтор через те же 2 минуты;
//! - снят: состояние = слепок, пик = слепок (минус плановые затраты возврата в пути — drawdown.rs fresh), пауза
//!   направления по просадке снимается; журнал core.drawdown_snap; запрошен человеком или снята пауза — Telegram
//!   risk.daily_limit_reset.
//!
//! Кто вызывает: engine.rs (run — запуск и наблюдатель настроек), day_roll.rs, daily_venue_limit.rs, daily_limit.rs
//! (request); тесты (apply — на записанных сделках учёта, факт счёта — числом, как remain_fact.rs apply_fact).
//! От чего зависит: drawdown.rs, drawdown_cfg.rs, drawdown_fact.rs, drawdown_ledger.rs, daily.rs (today_book, commit),
//! daily_venue.rs (days), daily_commit.rs (announce), day_pair.rs, deps.rs (журнал, уведомления, наблюдатель настроек).
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd (наблюдатель); venue.quote_asset (через drawdown_fact.rs).

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast::error::RecvError;

use crate::notify::human::{INFO, Msg, usd};
use crate::storage::journal::Level;

use super::day_pair::label;
use super::drawdown::{CLEAN_WAIT, Snap, SnapWay, Want};
use super::drawdown_fact::Place;
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Что делает: запрос нового слепка пар (только у пар с заданными risk.drawdown_*): отметка в книге дня, задача на
/// каждую пару (если её задача ещё не идёт). Принимает: ядро, пары, кто/почему, человек ли. Возвращает: строку для
/// текста сброса («» — таких пар нет). Ошибок нет.
pub async fn request(core: &Core, pairs: &[String], by: &str, human: bool) -> String {
    let pairs: Vec<String> = pairs
        .iter()
        .filter(|p| super::drawdown_cfg::active(core, p))
        .cloned()
        .collect();
    if pairs.is_empty() {
        return String::new();
    }
    let (today, mut book) = super::daily::today_book(core);
    let by = if human {
        format!("«Сбросить лимит» ({by})")
    } else {
        by.to_string()
    };
    for p in &pairs {
        let w = Want {
            at: Utc::now(),
            by: by.clone(),
            human,
            note: None,
        };
        book.draw_want.insert(p.clone(), w);
    }
    let trip = super::daily::commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
    for p in &pairs {
        spawn(core, p.clone());
    }
    let names: Vec<String> = pairs.iter().map(|p| label(p)).collect();
    format!(
        " Новый слепок баланса снимается: {} — сразу, как у направления не будет идущих сделок (не дольше 2 минут, \
         дальше — по учёту сделок); пауза по просадке снимется, когда слепок снят (решение 414).",
        names.join(", ")
    )
}

/// Запуск задачи слепка пары, если её ещё нет (Inner.draw_busy). Ошибок нет.
fn spawn(core: &Core, p: String) {
    if !lk(&core.inner.draw_busy).insert(p.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        take(&core, &p).await;
        lk(&core.inner.draw_busy).remove(&p);
    });
}

/// Слепок пары ещё запрошен (и пара по просадке). Ошибок нет.
fn wanted(core: &Core, p: &str) -> bool {
    super::drawdown_cfg::active(core, p) && lk(&core.inner.risk).day.draw_want.contains_key(p)
}

/// Задача пары: ждать чистого момента (до 2 минут), прочитать факт, снять; не прочитался — причина и повтор через
/// 2 минуты. Ошибок нет.
async fn take(core: &Core, p: &str) {
    while wanted(core, p) {
        wait_clean(core, p).await;
        match once(core, p).await {
            Ok(_) => return,
            Err(e) => {
                note(core, p, &e).await;
                tokio::time::sleep(CLEAN_WAIT).await;
            }
        }
    }
}

/// Ждёт, пока у пары нет идущих сделок, не дольше CLEAN_WAIT; будится сохранением сделок (живой поток ядра).
/// Возвращает: true — дождались. Ошибок нет.
async fn wait_clean(core: &Core, p: &str) -> bool {
    let end = tokio::time::Instant::now() + CLEAN_WAIT;
    let mut rx = core.subscribe();
    // Учёт перечитывается только после сохранения сделки (или пропуска событий), а не на каждое событие потока.
    let mut dirty = true;
    loop {
        if dirty && super::drawdown_ledger::active(core, p).is_empty() {
            return true;
        }
        tokio::select! {
            r = rx.recv() => {
                dirty = matches!(r, Ok(CoreEvent::Deal(_)) | Err(RecvError::Lagged(_)));
                if matches!(r, Err(RecvError::Closed)) {
                    tokio::time::sleep_until(end).await;
                    return super::drawdown_ledger::active(core, p).is_empty();
                }
            }
            _ = tokio::time::sleep_until(end) => {
                return super::drawdown_ledger::active(core, p).is_empty();
            }
        }
    }
}

/// Одно снятие: площадка покупки, факт USDT, слепок. Ошибки: текст (нет кошелька, не задана котируемая монета, сбой
/// балансов).
async fn once(core: &Core, p: &str) -> Result<Snap, String> {
    let pl = super::drawdown_fact::place(core, p)?;
    let q = super::drawdown_fact::quote(core, &pl.venue)?;
    let fact = super::drawdown_fact::usdt(core, &pl).await?;
    Ok(apply(core, p, &pl, &q, fact).await)
}

/// Что делает: слепок по прочитанному факту: своё и чужое по учёту (drawdown_ledger.rs count), слепок = факт + своё −
/// чужое; состояние пары = слепок, пик = слепок − плановые затраты возврата; запрос снят; пауза по просадке снята.
/// Принимает: ядро, id пары, площадку покупки, котируемую монету, факт USDT. Возвращает: слепок. Ошибок нет.
pub async fn apply(core: &Core, p: &str, pl: &Place, quote: &str, fact: Decimal) -> Snap {
    let c = super::drawdown_ledger::count(core, p, pl, quote);
    let (today, mut book) = super::daily::today_book(core);
    let want = book.draw_want.remove(p);
    let snap = Snap {
        base_usd: fact + c.own_usd - c.foreign_usd,
        fact_usd: fact,
        own_usd: c.own_usd,
        foreign_usd: c.foreign_usd,
        way: if c.active.is_empty() {
            SnapWay::Clean
        } else {
            SnapWay::Ledger
        },
        at: Utc::now(),
        day: today,
        by: want
            .as_ref()
            .map_or_else(|| "слепок".into(), |w| w.by.clone()),
        place: super::drawdown_fact::words(core, pl),
        own: c.own,
        foreign: c.foreign,
    };
    let seen = super::daily_venue::days(core, &book, today)
        .get(p)
        .map_or(Decimal::ZERO, |x| x.result_usd);
    let st = super::drawdown::fresh(snap.clone(), seen, today, c.pending_cost);
    book.draw.insert(p.to_string(), st);
    let lifted = {
        let mut r = lk(&core.inner.risk);
        r.draw_seen.remove(p);
        r.draw_paused.remove(p) && r.pair_paused.remove(p).is_some()
    };
    let trip = super::daily::commit(core, today, book).await;
    let human = want.is_some_and(|w| w.human);
    report(core, p, &snap, lifted, human).await;
    super::daily_commit::announce(core, &trip).await;
    snap
}

/// Журнал core.drawdown_snap; Telegram risk.daily_limit_reset — если запросил человек или снята пауза. Ошибок нет.
async fn report(core: &Core, p: &str, s: &Snap, lifted: bool, human: bool) {
    let deals = |v: &[(String, Decimal)]| {
        v.iter()
            .map(|(id, a)| format!("{id} {}", usd(*a)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "Слепок баланса направления {} снят ({}; {}): на {} по факту {}",
        label(p),
        super::drawdown_limit::way_text(s.way),
        s.by,
        s.place,
        usd(s.fact_usd)
    );
    if !s.own.is_empty() {
        text.push_str(&format!(
            " + своё в пути {} ({})",
            usd(s.own_usd),
            deals(&s.own)
        ));
    }
    if !s.foreign.is_empty() {
        text.push_str(&format!(
            " − чужое на счёте {} ({})",
            usd(s.foreign_usd),
            deals(&s.foreign)
        ));
    }
    text.push_str(&format!(
        " = {}. Пик = слепок (решение 414)",
        usd(s.base_usd)
    ));
    if lifted {
        text.push_str("; пауза по просадке снята — направление снова торгует");
    }
    let v = super::day_pair::venue_or_dex(core, p);
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.drawdown_snap", v.as_deref(), &text, None)
        .await;
    if human || lifted {
        let tg = Msg::new(INFO, format!("Новый слепок: {}", label(p)))
            .line(format!("Слепок {} — он же пик", usd(s.base_usd)))
            .line(super::drawdown_limit::way_text(s.way))
            .line(if lifted {
                "Пауза по просадке снята — направление торгует"
            } else {
                ""
            })
            .render();
        deps.notify("risk.daily_limit_reset", v.as_deref(), &tg)
            .await;
    }
}

/// Слепок не снят: причина в запросе (экран) и журнал (только при смене причины). Ошибок нет.
async fn note(core: &Core, p: &str, why: &str) {
    let (today, mut book) = super::daily::today_book(core);
    let Some(w) = book.draw_want.get_mut(p) else {
        return;
    };
    if w.note.as_deref() == Some(why) {
        return;
    }
    w.note = Some(why.to_string());
    let trip = super::daily::commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
    let text = format!(
        "Слепок баланса направления {} не снят: {why} — действует прежний пик; повтор через 2 минуты (решение 414)",
        label(p)
    );
    let v = super::day_pair::venue_or_dex(core, p);
    core.inner
        .deps
        .log(Level::Warn, "core.drawdown_snap", v.as_deref(), text, None)
        .await;
}

/// Что делает: запуск (после загрузки учёта) и наблюдатель настроек risk.drawdown_*: пересчёт итога дня (плановые
/// затраты возврата — по загруженным сделкам), слепки пар без слепка этого дня и недоснятые запросы. Работает, пока
/// жива система. Ошибок нет.
pub async fn run(core: Core) {
    // Пересчёт — только если есть пары по просадке (у пар по старому лимиту дня запуск — как раньше).
    let any = super::day_pair::all(&core)
        .iter()
        .any(|p| super::drawdown_cfg::active(&core, p));
    if any {
        let (today, book) = super::daily::today_book(&core);
        let trip = super::daily::commit(&core, today, book).await;
        super::daily_commit::announce(&core, &trip).await;
    }
    need(&core, "запуск").await;
    let mut w = core.inner.deps.watch(&[super::drawdown_cfg::PREFIX]);
    while w.next().await.is_some() {
        // Пересчёт: новые пределы действуют сразу (пауза или её снятие, если пределы убрали — drawdown_limit.rs).
        let (today, book) = super::daily::today_book(&core);
        let trip = super::daily::commit(&core, today, book).await;
        super::daily_commit::announce(&core, &trip).await;
        need(&core, "заданы пределы просадки risk.drawdown_*").await;
    }
}

/// Пары по просадке: недоснятый запрос — задача заново; слепка этого дня нет — новый запрос. Ошибок нет.
async fn need(core: &Core, by: &str) {
    let today = Utc::now().date_naive();
    let (draw, want) = {
        let r = lk(&core.inner.risk);
        (r.day.draw.clone(), r.day.draw_want.clone())
    };
    let mut ask = Vec::new();
    for p in super::day_pair::all(core) {
        if !super::drawdown_cfg::active(core, &p) {
            continue;
        }
        if want.contains_key(&p) {
            spawn(core, p);
        } else if draw.get(&p).is_none_or(|s| s.snap.day != today) {
            ask.push(p);
        }
    }
    request(core, &ask, by, false).await;
}
