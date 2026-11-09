//! Файл: core/uncapped_maxwin.rs
//! Что делает: решение 245 (часть Б, Тигран 09.11: согласовано) — «окно максимального объёма» кандидата: серия расчётов
//! «без потолка» (решение 141) одного направления подряд, в которых оптимум без потолка (сделка максимального объёма,
//! без бюджета и баланса) проходит пороги прибыли (Record.opt_passes), с промежутками не длиннее
//! uncapped.maxwin_gap_ms. Начало — время первого такого расчёта (Record.at); конец — время первого расчёта, где оптимум
//! пороги НЕ проходит (end «not_passing», длительность точная с шагом расчётов); расчётов нет дольше
//! uncapped.maxwin_gap_ms — конец неизвестен (end «gap», длительность — «не меньше»: до последнего выгодного расчёта).
//! Объём и прибыль — на начале и в пике (наибольшая прибыль оптимума за окно).
//! Дополнение (Тигран 09.11: согласовано, uncapped_watch.rs): при uncapped.maxwin_watch_ms окно наблюдается и вне
//! кандидатов (расчёты наблюдения); выгодный расчёт через ≥ uncapped.maxwin_watch_ms от начала закрывает окно как
//! «watch» (держалось всё наблюдение, длительность «не меньше времени наблюдения», поле watch_ms), и пока направление
//! остаётся выгодным (выгодные расчёты без промежутка длиннее uncapped.maxwin_gap_ms), новое окно не начинается
//! (иначе одно долгое окно дробилось бы на цепочку «2 м+»); невыгодный расчёт или промежуток снимают это. Расчёт
//! наблюдения новое окно не начинает (начало — только расчёт кандидата).
//! Хранение: тело записи эпизода кандидата (SQLite uncapped, поле `maxwin`) — у эпизода самое длинное окно, последнее
//! и сколько окон пересеклось с эпизодом; сводка окна решения 125 (поле `maxwins` по направлениям, если окно монеты
//! открыто и направление в нём). Старые записи — без поля (пусто), не пересчитываются.
//! Кто вызывает: core/uncapped_store.rs (note, apply, attach — на каждый расчёт кандидата), maxwin_view.rs (MaxWins).
//! От чего зависит: uncapped_state.rs (память по направлениям, открытые эпизоды), uncapped_types.rs (Record),
//! size_uncapped.rs (At), window.rs (dex_slot — окно монеты с направлением), storage/uncapped_db.rs, settings.
//! Настройки: uncapped.maxwin_gap_ms (не задана — окна максимального объёма не ведутся), uncapped.maxwin_watch_ms
//! (не задана — без закрытия «watch», как до дополнения).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::types::{Direction, VenueId};
use crate::settings::read::At as SAt;
use crate::storage::uncapped_db;

use super::engine::{Core, lk};
use super::size_uncapped::At;
use super::uncapped_types::Record;

/// Окно максимального объёма.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaxWin {
    pub start_at: DateTime<Utc>,
    /// Оптимум без потолка на начале: объём, вложить, прибыль.
    pub start: At,
    pub peak_at: DateTime<Utc>,
    pub peak: At,
    /// Последний расчёт, где оптимум проходил пороги.
    pub last_pass_at: DateTime<Utc>,
    /// Первый расчёт, где оптимум пороги не прошёл; None — конец не наблюдался.
    pub end_at: Option<DateTime<Utc>>,
    /// «not_passing» — перестала быть выгодной (end_at); «gap» — расчётов не было дольше uncapped.maxwin_gap_ms;
    /// «watch» — держалось всё время наблюдения uncapped.maxwin_watch_ms (дополнение 09.11); None — окно шло на момент
    /// последнего расчёта.
    pub end: Option<String>,
    /// Выгодных расчётов в окне.
    pub evals: u64,
    /// Конец «watch»: время наблюдения, мс (на экране «≥ …», «2 м+»). Нет — у старых записей и других концов.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_ms: Option<u64>,
}

impl MaxWin {
    /// Длительность, мс: точная (до end_at) или «не меньше» (до последнего выгодного расчёта). Ошибок нет.
    pub fn ms(&self) -> i64 {
        (self.end_at.unwrap_or(self.last_pass_at) - self.start_at)
            .num_milliseconds()
            .max(0)
    }
}

/// Окна максимального объёма записи: самое длинное, последнее и сколько их было.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaxWins {
    pub best: MaxWin,
    pub last: MaxWin,
    pub n: u64,
}

/// Окна максимального объёма направления в сводке окна решения 125.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirMaxWins {
    pub buy_on: VenueId,
    pub sell_on: VenueId,
    pub wins: MaxWins,
}

/// Память направления: окно, которое идёт (промежуток считается от его последнего выгодного расчёта: невыгодный
/// расчёт окно закрывает, поэтому подряд в окне идут только выгодные); после закрытия «watch» — время последнего
/// выгодного расчёта, пока направление остаётся выгодным (новое окно не начинается).
#[derive(Debug, Clone, Default)]
pub struct Track {
    cur: Option<MaxWin>,
    held: Option<DateTime<Utc>>,
}

impl Track {
    /// Начало окна, которое идёт (uncapped_watch.rs); None — окна нет. Ошибок нет.
    pub fn going(&self) -> Option<DateTime<Utc>> {
        self.cur.as_ref().map(|c| c.start_at)
    }
}

/// Что делает: окно идёт или кончилось — в MaxWins (тот же старт — то же окно; самое длинное — по ms). Принимает:
/// прежние окна записи (None — не было), окно. Возвращает: новые. Ошибок нет.
pub fn merge(old: Option<MaxWins>, w: &MaxWin) -> MaxWins {
    let Some(mut s) = old else {
        let (best, last) = (w.clone(), w.clone());
        return MaxWins { best, last, n: 1 };
    };
    if s.last.start_at != w.start_at {
        s.n += 1;
    }
    s.last = w.clone();
    if s.best.start_at == w.start_at || w.ms() > s.best.ms() {
        s.best = w.clone();
    }
    s
}

/// Окна, пересекающиеся с записью, начатой в `first` (последний выгодный расчёт не раньше её начала). Ошибок нет.
pub fn near(wins: &[MaxWin], first: DateTime<Utc>) -> Vec<MaxWin> {
    wins.iter()
        .filter(|w| w.last_pass_at >= first)
        .cloned()
        .collect()
}

/// Что делает: расчёт кандидата (или наблюдения, `watch`) посчитан — шаг окна направления (см. шапку) и сводка окна
/// решения 125. Только память. Принимает: ядро, направление, запись, расчёт наблюдения ли (uncapped_watch.rs: новое
/// окно он не начинает). Возвращает: затронутые окна (кончившееся по промежутку и текущее/кончившееся); пусто —
/// uncapped.maxwin_gap_ms не задана или окна нет. Ошибок нет.
pub fn note(core: &Core, d: &Direction, rec: &Record, watch: bool) -> Vec<MaxWin> {
    let Some(gap) = gap_ms(core).ok() else {
        return vec![];
    };
    let gap = i64::try_from(gap).unwrap_or(i64::MAX);
    // Дополнение 09.11: время наблюдения (не задано — без закрытия «watch»).
    let limit = super::uncapped_watch::watch_ms(core).ok();
    let mut out = vec![];
    {
        let mut m = lk(&core.inner.uncapped.maxwin);
        let t = m.entry(d.clone()).or_default();
        if let Some(c) = t.cur.as_mut()
            && (rec.at - c.last_pass_at).num_milliseconds() > gap
        {
            c.end = Some("gap".into());
            out.extend(t.cur.take());
        }
        if t.held
            .is_some_and(|h| (rec.at - h).num_milliseconds() > gap)
        {
            t.held = None;
        }
        match (t.cur.as_mut(), rec.opt_passes) {
            // Держалось всё наблюдение и всё ещё выгодно — то же окно, новое не начинается.
            (None, true) if t.held.is_some() => t.held = Some(rec.at),
            (None, true) if watch => {}
            (None, true) => t.cur = Some(start(rec)),
            (Some(c), true) => {
                if rec.opt.profit > c.peak.profit {
                    (c.peak, c.peak_at) = (rec.opt, rec.at);
                }
                c.last_pass_at = rec.at;
                c.evals += 1;
                let held = limit.filter(|w| {
                    (rec.at - c.start_at).num_milliseconds()
                        >= i64::try_from(*w).unwrap_or(i64::MAX)
                });
                if let Some(w) = held {
                    (c.end, c.watch_ms) = (Some("watch".into()), Some(w));
                    t.held = Some(rec.at);
                    out.extend(t.cur.take());
                }
            }
            (Some(c), false) => {
                (c.end_at, c.end) = (Some(rec.at), Some("not_passing".into()));
                out.extend(t.cur.take());
            }
            (None, false) => t.held = None,
        }
        out.extend(t.cur.clone());
    }
    let mut w = lk(&core.inner.windows);
    if let Some((info, _)) = w.dex_slot(d) {
        for x in near(&out, info.started_at) {
            let i = info
                .maxwins
                .iter()
                .position(|m| m.buy_on == d.buy_on && m.sell_on == d.sell_on);
            let wins = merge(i.map(|i| info.maxwins[i].wins.clone()), &x);
            match i {
                Some(i) => info.maxwins[i].wins = wins,
                None => info.maxwins.push(DirMaxWins {
                    buy_on: d.buy_on.clone(),
                    sell_on: d.sell_on.clone(),
                    wins,
                }),
            }
        }
    }
    out
}

/// Новое окно с расчёта. Ошибок нет.
fn start(rec: &Record) -> MaxWin {
    MaxWin {
        start_at: rec.at,
        start: rec.opt,
        peak_at: rec.at,
        peak: rec.opt,
        last_pass_at: rec.at,
        end_at: None,
        end: None,
        evals: 1,
        watch_ms: None,
    }
}

/// Что делает: окна — в тело записи эпизода (поле `maxwin`). Принимает: тело, окна. Возвращает: менялось ли.
pub fn apply(body: &mut Value, wins: &[MaxWin]) -> bool {
    if wins.is_empty() {
        return false;
    }
    let mut slot: Option<MaxWins> = serde_json::from_value(body["maxwin"].clone()).ok();
    for w in wins {
        slot = Some(merge(slot, w));
    }
    body["maxwin"] = serde_json::to_value(&slot).unwrap_or_default();
    true
}

/// Что делает: расчёт без упущенного (в базу не пишется) — его окна в открытый эпизод направления (последняя строка
/// направления), если окно пересекается с эпизодом. Принимает: ядро, направление, окна, время расчёта.
/// Ошибок нет: сбой базы — в счётчик сбоев (uncapped_store::failed).
pub async fn attach(core: &Core, d: &Direction, wins: &[MaxWin], at: DateTime<Utc>) {
    let row = {
        let mut eps = lk(&core.inner.uncapped.episodes);
        let Some(op) = eps.get_mut(d) else { return };
        let first = serde_json::from_value(op.body["ep"]["first_at"].clone()).ok();
        let Some(first) = first else { return };
        apply(&mut op.body, &near(wins, first)).then(|| (op.id, op.body.to_string()))
    };
    if let Some((id, body)) = row {
        let ts = super::uncapped_store::ts(at);
        if let Err(e) = uncapped_db::update(&core.inner.deps.db, id, ts, body).await {
            super::uncapped_store::failed(core, format!("запись в базу: {e}"));
        }
    }
}

/// uncapped.maxwin_gap_ms. Ошибки: текст — не задана.
pub fn gap_ms(core: &Core) -> Result<u64, String> {
    core.inner
        .deps
        .settings
        .integer("uncapped.maxwin_gap_ms", SAt::default())
        .map_err(|e| e.message)
}
