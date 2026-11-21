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
//!   журнал, причина на экране (Want.note), повтор ровно через 2 минуты;
//! - снят: состояние = слепок, пик = слепок (минус плановые затраты возврата в пути — drawdown.rs fresh), пауза
//!   направления по просадке снимается; журнал core.drawdown_snap; запрошен человеком или снята пауза — Telegram
//!   risk.daily_limit_reset.
//!
//! - снят: снимается и пауза, поставленная старым risk.max_daily_loss_usd до перехода направления на просадку
//!   (ревью t199, С3); причина — в журнале и Telegram.
//!
//! Запросы и запуск задачи — drawdown_ask.rs (вынесено, ревью t199). Повтор после неудачного чтения — ровно через 2
//! минуты (чистого момента ждём один раз).
//! Кто вызывает: drawdown_ask.rs (take); тесты (apply — на записанных сделках учёта, факт счёта — числом, как
//! remain_fact.rs apply_fact).
//! От чего зависит: drawdown.rs, drawdown_cfg.rs, drawdown_fact.rs, drawdown_ledger.rs, daily.rs (today_book, commit),
//! daily_venue.rs (days), daily_commit.rs (announce), day_pair.rs, deps.rs (журнал, уведомления, наблюдатель настроек).
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd (наблюдатель); venue.quote_asset (через drawdown_fact.rs).

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::broadcast::error::RecvError;

use super::drawdown::{CLEAN_WAIT, Snap, SnapWay};
use super::drawdown_fact::Place;
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Задача пары (запускает drawdown_ask.rs): ждать чистого момента (до 2 минут), прочитать факт, снять; не
/// прочитался — причина и повтор ровно через 2 минуты. Ошибок нет.
pub(super) async fn take(core: &Core, p: &str) {
    // Чистого момента ждём один раз; повтор после неудачного чтения — ровно через 2 минуты (ревью t199, мелкое 4),
    // слепок тогда — по учёту, если сделки идут.
    wait_clean(core, p).await;
    while super::drawdown_ask::wanted(core, p) {
        match once(core, p).await {
            Ok(_) => return,
            Err(e) => {
                super::drawdown_note::note(core, p, &e).await;
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
    // Пауза направления снимается любая из пауз по убытку: по просадке и (ревью t199, С3 — переход со старого лимита)
    // поставленная старым risk.max_daily_loss_usd до того, как направление перешло на просадку.
    let lifted = {
        let mut r = lk(&core.inner.risk);
        r.draw_seen.remove(p);
        r.draw_seen_limit.remove(p);
        r.loss_seen.remove(p);
        let draw = r.draw_paused.remove(p);
        match r.pair_paused.remove(p) {
            Some(_) if draw => Some("по просадке"),
            Some(_) => Some("по старому лимиту дня (risk.max_daily_loss_usd)"),
            None => None,
        }
    };
    let trip = super::daily::commit(core, today, book).await;
    let human = want.is_some_and(|w| w.human);
    super::drawdown_note::report(core, p, &snap, lifted, human).await;
    super::daily_commit::announce(core, &trip).await;
    snap
}
