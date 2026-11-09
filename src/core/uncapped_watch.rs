//! Файл: core/uncapped_watch.rs
//! Что делает: решение 245, дополнение (Тигран 09.11: согласовано) — постоянное наблюдение окна максимального объёма
//! (uncapped_maxwin.rs). Тигран: «если записываем вслепую, т.е. самой сделки нет — постоянно записывать 2 мин: если в
//! течение 2 мин держится, например, 38 секунд — пишем 38; если больше 2 мин — пишем „2 м+“». С начала окна (первый
//! расчёт кандидата, где оптимум без потолка проходит пороги) направление наблюдается uncapped.maxwin_watch_ms НЕЗАВИСИМО
//! от того, остаётся ли оно кандидатом: после каждого расчёта ключа кандидата направления (uncapped_queue.rs run), пока
//! окно идёт, ставится расчёт наблюдения (submit_watch) — та же очередь, тот же ключ, интервал uncapped.min_interval_ms
//! (от начала прошлого расчёта ключа), предел uncapped.queue_max, место uncapped.max_concurrent; ожидающий расчёт
//! кандидата он не вытесняет, а расчёт кандидата вытесняет его — поэтому по направлению не больше одного расчёта на
//! интервал, кандидат он или нет. Конец наблюдения — окно закрылось (первый невыгодный расчёт; держалось всё время
//! наблюдения — uncapped_maxwin.rs закрывает его как «watch»; промежуток длиннее uncapped.maxwin_gap_ms) или с начала
//! окна прошло больше uncapped.maxwin_watch_ms + uncapped.maxwin_gap_ms (расчёты не шли или сбоили — защита).
//! Бой расчёта наблюдения — бой прошлого расчёта направления (бюджет и составляющие тех же); итог — только окно
//! (uncapped_maxwin::note с пометкой «наблюдение» и attach к открытому эпизоду): строка «Кандидатов», запись эпизода
//! «Упущено» и счётчик «без упущенного» его не получают — упущенное по-прежнему меряют только расчёты кандидата.
//! Монета занята нашей сделкой (inner.active, решение 118) — расчёта наблюдения нет, как у кандидата (candidates.rs
//! enqueue не ставит её в расчёт; живые стакан и пулы двигает наша же сделка): цепочка обрывается, окно кончится по
//! промежутку uncapped.maxwin_gap_ms (длительность «не меньше»).
//! Кто вызывает: core/uncapped_queue.rs (next — после расчёта ключа кандидата; saved — итог расчёта наблюдения).
//! От чего зависит: uncapped_queue.rs (submit_watch), uncapped_maxwin.rs (note, attach, Track), uncapped_state.rs,
//! coin_key.rs, settings.
//! Настройки: uncapped.maxwin_watch_ms (не задана — наблюдения нет), uncapped.maxwin_gap_ms.

use chrono::Utc;

use crate::core::types::Direction;
use crate::settings::read::At;

use super::engine::{Core, lk};
use super::uncapped_maxwin as maxwin;
use super::uncapped_types::{Battle, Record};

/// uncapped.maxwin_watch_ms. Ошибки: текст — не задана.
pub fn watch_ms(core: &Core) -> Result<u64, String> {
    core.inner
        .deps
        .settings
        .integer("uncapped.maxwin_watch_ms", At::default())
        .map_err(|e| e.message)
}

/// Что делает: после расчёта ключа кандидата направления — если окно направления наблюдается (см. шапку), ставит
/// следующий расчёт наблюдения. Принимает: ядро, направление, бой прошлого расчёта. Ошибок нет: настройка не задана,
/// окна нет, наблюдение кончилось, монета занята — ничего не ставится.
pub fn next(core: &Core, d: &Direction, battle: Battle) {
    let (Ok(watch), Ok(gap)) = (watch_ms(core), maxwin::gap_ms(core)) else {
        return;
    };
    let Some(start) = lk(&core.inner.uncapped.maxwin)
        .get(d)
        .and_then(maxwin::Track::going)
    else {
        return;
    };
    let limit = i64::try_from(watch.saturating_add(gap)).unwrap_or(i64::MAX);
    if (Utc::now() - start).num_milliseconds() > limit {
        return;
    }
    // Решение 118: по монете идёт наша сделка — расчётов нет, как у кандидата.
    if lk(&core.inner.active).contains(&super::coin_key::CoinKey::of(&d.asset)) {
        return;
    }
    super::uncapped_queue::submit_watch(core, d, battle);
}

/// Что делает: итог расчёта наблюдения — только окно максимального объёма (note с пометкой «наблюдение»: новое окно
/// он не начинает) и окна в открытый эпизод направления (attach); счётчики «посчитано» и «наблюдение».
/// Принимает: ядро, направление, запись. Ошибок нет: сбой базы — в счётчик сбоев (attach).
pub async fn saved(core: &Core, d: &Direction, rec: Record) {
    {
        let mut s = lk(&core.inner.uncapped.stats);
        s.done += 1;
        s.watched += 1;
        s.total_ms += rec.ms;
        s.max_ms = s.max_ms.max(rec.ms);
    }
    let wins = maxwin::note(core, d, &rec, true);
    maxwin::attach(core, d, &wins, rec.at).await;
}
