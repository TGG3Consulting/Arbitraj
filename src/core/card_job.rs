//! Файл: core/card_job.rs
//! Что делает: карточка сигнала (решение 74) строится ВНЕ очереди расчёта кандидатов — своей задачей с классом
//! срочности Quote, следующий кандидат её не ждёт. По направлению одновременно строится одна карточка; пришёл новый
//! прошедший, пока строится, — он ждёт следующим (более ранний ожидающий заменяется, отметка «первая запись эпизода»
//! того же эпизода сохраняется), а если записи эпизода не нужно (пропуск по интервалу) — не ставится вовсе.
//! Готовая карточка — в строку «Кандидатов» и живой поток; запись эпизода — сообщением задаче эпизодов
//! (episode_worker.rs); эпизоды не ведутся — журнал и Telegram deal.candidate отсюда (told).
//! Кто вызывает: core/candidates.rs (submit).
//! От чего зависит: card.rs, card_text.rs, episode_run.rs (EpMsg), view.rs, gateway/urgency.rs.
//! Настройки: через card.rs.

use crate::core::types::Direction;
use crate::gateway::urgency::{self, Urgency};
use crate::storage::journal::Level;

use super::costs::Calc;
use super::engine::{Core, lk};
use super::episode_run::{Accepted, EpMsg, send};
use super::view::CoreEvent;

/// Работа: расчёт прошедшего кандидата и что с карточкой делать.
pub struct Job {
    pub calc: Calc,
    pub dry: bool,
    /// Запись эпизода (Accepted::Ep) или ничего.
    pub ep: Accepted,
    /// Эпизоды не ведутся — сообщить карточку в журнал и Telegram отсюда.
    pub told: bool,
    /// Добавка к тексту сообщения (например «сухой режим: …»).
    pub note: Option<String>,
}

/// Что делает: ставит карточку в работу. Принимает: ядро, работу, ставить ли ожидающей, если по направлению уже
/// строится. Ошибок нет.
pub fn submit(core: &Core, job: Job, wait: bool) {
    let d = job.calc.direction.clone();
    {
        let mut jobs = lk(&core.inner.card_jobs);
        if let Some(pending) = jobs.get_mut(&d) {
            if wait {
                *pending = Some(merge(pending.take(), job));
            }
            return;
        }
        jobs.insert(d.clone(), None);
    }
    let core = core.clone();
    tokio::spawn(async move {
        let mut job = job;
        loop {
            urgency::scope(Urgency::Quote, run(&core, job)).await;
            let mut jobs = lk(&core.inner.card_jobs);
            match jobs.get_mut(&d).and_then(Option::take) {
                Some(next) => job = next,
                None => {
                    jobs.remove(&d);
                    return;
                }
            }
        }
    });
}

/// Новая ожидающая работа вместо прежней; «первая запись» того же эпизода и told не теряются. Ошибок нет.
fn merge(old: Option<Job>, mut new: Job) -> Job {
    let Some(old) = old else { return new };
    if let (
        Accepted::Ep {
            info: a, first: fa, ..
        },
        Accepted::Ep { info: b, first, .. },
    ) = (&old.ep, &mut new.ep)
        && a.id == b.id
    {
        *first |= *fa;
    }
    new.told |= old.told;
    new
}

/// Строит карточку и раздаёт её. Ошибок нет.
async fn run(core: &Core, job: Job) {
    let card = super::card::build(core, &job.calc, job.dry).await;
    publish(core, &job.calc.direction, &card);
    if job.told {
        let mut text = super::card_text::text(&card);
        if let Some(s) = job.calc.size.as_ref() {
            text.push_str(&format!("\nОбъём: {}", s.why));
        }
        if let Some(n) = &job.note {
            text.push_str(&format!("\n{n}"));
        }
        let deps = &core.inner.deps;
        let v = job.calc.direction.buy_on.0.clone();
        deps.log(
            Level::Info,
            "core.candidate",
            Some(&v),
            &text,
            serde_json::to_value(&job.calc).ok(),
        )
        .await;
        deps.notify("deal.candidate", Some(&v), &text).await;
    }
    if let Accepted::Ep { info, snap, .. } = job.ep {
        send(
            core,
            EpMsg::Record {
                info: *info,
                calc: Box::new(job.calc),
                snap,
                card: Some(Box::new(card)),
            },
        );
    }
}

/// Карточка — в строку «Кандидатов» направления (если там всё ещё прошедший) и живой поток. Ошибок нет.
fn publish(core: &Core, d: &Direction, card: &super::card::Card) {
    let row = {
        let mut rows = lk(&core.inner.candidates);
        let Some(row) = rows.get_mut(d) else { return };
        if row["verdict"].get("Accept").is_none() {
            return;
        }
        row["card"] = serde_json::json!(card);
        row.clone()
    };
    core.inner.live.send(CoreEvent::Candidate(row));
}
