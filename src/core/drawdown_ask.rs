//! Файл: core/drawdown_ask.rs
//! Что делает: решение 414 — запросы слепка баланса направлений (вынесено из drawdown_snap.rs): кто и когда просит
//! новый слепок и запуск задачи снятия (drawdown_snap.rs take) — по одной на пару:
//! - «Сбросить лимит дня» по направлению / площадке / общий (daily_venue_limit.rs, daily_limit.rs), новый день UTC
//!   (day_roll.rs) — request;
//! - запуск (после загрузки учёта): пересчёт итога дня, недоснятые запросы — задача заново, нет слепка этого дня —
//!   новый запрос; наблюдатель настроек risk.drawdown_* — то же (направление перешло на просадку) и пересчёт — run
//!   (снизили предел направления ниже просадки — пауза сразу: drawdown_limit.rs по Risk.draw_seen_limit).
//!
//! Запрос (Want) пишется в книгу дня (переживает перезапуск). Задача пары одна (Inner.draw_busy); ревью t199, С2:
//! задача, закончив, снимает отметку и проверяет, не пришёл ли за это время новый запрос, — тогда снимает ещё раз
//! (запрос «Сбросить лимит», пришедший, пока прежняя задача заканчивалась, не теряется).
//! Кто вызывает: engine.rs (run), day_roll.rs, daily_venue_limit.rs, daily_limit.rs (request), drawdown_snap.rs (wanted).
//! От чего зависит: drawdown_snap.rs (take), drawdown_cfg.rs, daily.rs (today_book, commit), daily_commit.rs
//! (announce), day_pair.rs, deps.rs (наблюдатель настроек).
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd (наблюдатель).

use chrono::Utc;

use super::day_pair::label;
use super::drawdown::Want;
use super::engine::{Core, lk};

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

/// Что делает: запуск задачи слепка пары, если её ещё нет (Inner.draw_busy). Задача, закончив, снимает отметку и,
/// если за это время пришёл новый запрос, снимает слепок снова (С2). Принимает: ядро, пару. Ошибок нет.
pub(super) fn spawn(core: &Core, p: String) {
    if !lk(&core.inner.draw_busy).insert(p.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        loop {
            super::drawdown_snap::take(&core, &p).await;
            lk(&core.inner.draw_busy).remove(&p);
            // Новый запрос пришёл, пока задача заканчивалась (spawn тогда отказал по отметке), — снова; если отметку
            // уже взяла другая задача — она и снимет.
            if !wanted(&core, &p) || !lk(&core.inner.draw_busy).insert(p.clone()) {
                break;
            }
        }
    });
}

/// Слепок пары ещё запрошен (и пара по просадке). Принимает: ядро, пару. Ошибок нет.
pub(super) fn wanted(core: &Core, p: &str) -> bool {
    super::drawdown_cfg::active(core, p) && lk(&core.inner.risk).day.draw_want.contains_key(p)
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
        super::drawdown_limit::save(&core).await;
    }
    need(&core, "запуск").await;
    let mut w = core.inner.deps.watch(&[super::drawdown_cfg::PREFIX]);
    while w.next().await.is_some() {
        // Финальное ревью t199: замеченная просадка не забывается (иначе смена настройки одного направления снова
        // ставила бы на паузу другое после «Снять паузу»); снижение предела направления drawdown_limit.rs видит по
        // Risk.draw_seen_limit — пауза сразу.
        // Пересчёт: новые пределы действуют сразу (пауза или её снятие, если пределы убрали — drawdown_limit.rs).
        super::drawdown_limit::save(&core).await;
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
