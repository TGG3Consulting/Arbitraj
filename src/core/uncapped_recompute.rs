//! Файл: core/uncapped_recompute.rs
//! Что делает: решение 218 (Тигран, 07.11.2026) — пересчёт СТАРЫХ записей «без потолка» по исполненным сделкам,
//! посчитанных после нашей покупки (basis live_after_buy — занижены, решение 217), по записанному окну сделки
//! (решение 125). Запускается один раз внешним инструментом (src/bin/uncapped_recompute.rs). Модель и поиск — те же
//! функции ядра, что у живого расчёта (costs.rs evaluate_with → size_uncapped.rs uncapped → uncapped_record.rs record);
//! данные — источник uncapped_replay.rs (стаканы окна, план сделки). Второй модели нет.
//! Правила (утверждены Тиграном):
//! - пересчитывается сделка, у которой ПОСЛЕДНЯЯ запись — старая (live_after_buy); уже пересчитанная — нет;
//! - момент решения — время первого события сделки «План…» (иначе started_at);
//! - окно — то, в котором записана сделка, открытое до момента решения; иначе — не пересчитывается;
//! - биржа ↔ биржа: обе стороны — последний полный стакан каждой биржи окна строго до момента решения (window_at.rs);
//!   нет полного стакана — не пересчитывается (причина);
//! - биржа → DEX: биржа — так же; DEX — из плана сделки (приблизительно, basis recomputed_window_dex_plan);
//! - DEX → биржа, циклы DEX ↔ DEX, сделки без окна — не пересчитываются (причина в отчёт).
//!
//! Итог по сделке — строка отчёта и (если посчитано) новая строка таблицы uncapped: тот же формат Record, basis
//! recomputed_window / recomputed_window_dex_plan, начало — как у старой записи (тот же период на экране). Старые
//! записи не удаляются и не меняются; основная запись сделки — последняя (uncapped_db by_deal / by_deals,
//! uncapped_sum.rs latest_per_deal). Контекст направления — uncapped_recompute_ctx.rs.
//! Кто вызывает: core/uncapped_recompute_all.rs (программа src/bin/uncapped_recompute.rs), тесты (tests/uncapped_recompute.rs).
//! От чего зависит: uncapped_recompute_ctx.rs, uncapped_replay.rs, window_at.rs, size_curve.rs, size_uncapped.rs,
//! uncapped_record.rs, card_max.rs (passes), deal.rs, window.rs (WindowInfo), storage (uncapped_db, episode_files).
//! Настройки: card.max_search_usdt, money.min_profit_usd, money.min_profit_pct, uncapped.max_evals,
//! uncapped.ladder_usdt, size.*, venue.quote_asset, profit.price_move_k, profit.liquidity_share_h (как у живого
//! расчёта; не задана — сделка не пересчитывается, причина).

use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::settings::store::Store;
use crate::storage::uncapped_db::{DealRow, Row};

use super::deal::Deal;
use super::uncapped_recompute_ctx::{Old, Prepared, prepare};
use super::uncapped_record::{Inputs, record};
use super::uncapped_types::{BASIS_LIVE_AFTER_BUY, old_basis};
use super::window::WindowInfo;
use super::window_at::Decode;

/// Окружение пересчёта: настройки базы бота, папка файлов окон, разбор кадров адаптерами.
pub struct Env<'a> {
    pub settings: &'a Store,
    pub windows_dir: &'a Path,
    pub decode: Decode<'a>,
}

/// Группа строки отчёта.
pub const G_CEX: &str = "биржа ↔ биржа";
pub const G_DEX: &str = "биржа → DEX (DEX из плана)";
pub const G_SKIP: &str = "не пересчитано";

/// Строка отчёта по сделке.
#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub deal_id: String,
    /// «MCRT bybit → bsc_dex».
    pub direction: String,
    pub group: &'static str,
    /// Было (старая запись): упущено и потолок (opt.invested).
    pub was_missed: Option<Decimal>,
    pub was_cap: Option<Decimal>,
    /// Стало (новая запись).
    pub now_missed: Option<Decimal>,
    pub now_cap: Option<Decimal>,
    /// Откуда данные новой записи (стаканы окна, приближения).
    pub source: String,
    /// Почему не пересчитано.
    pub reason: Option<String>,
}

/// Итог по сделке: строка отчёта и новая строка базы (None — не пересчитано).
pub struct Item {
    pub line: Line,
    pub row: Option<Row>,
}

/// Строка «не пересчитано» без расчёта (причину ставит вызывающий). Ошибок нет.
pub(super) fn one_skip(row: &DealRow, deal: Option<&Deal>) -> Item {
    let old = Old::parse(&row.body).ok();
    Item {
        line: Line {
            deal_id: row.deal_id.clone(),
            direction: deal.map(dir_text).unwrap_or_default(),
            group: G_SKIP,
            was_missed: old.as_ref().map(|o| o.missed),
            was_cap: old.as_ref().map(|o| o.opt.invested),
            now_missed: None,
            now_cap: None,
            source: String::new(),
            reason: None,
        },
        row: None,
    }
}

/// Что делает: какие записи пересчитывать — последняя запись каждой сделки (по id строки). Принимает: все записи
/// сделок (uncapped_db::deal_rows). Возвращает: (последняя запись, нужно ли пересчитывать: она старая —
/// live_after_buy) по сделкам в порядке id. Ошибок нет (тело не читается — считается старым, причина будет при разборе).
pub fn select(rows: Vec<DealRow>) -> Vec<(DealRow, bool)> {
    let mut last: Vec<DealRow> = vec![];
    for r in rows {
        match last.iter_mut().find(|x| x.deal_id == r.deal_id) {
            Some(x) if x.id < r.id => *x = r,
            Some(_) => {}
            None => last.push(r),
        }
    }
    last.sort_by_key(|r| r.id);
    last.into_iter()
        .map(|r| {
            let basis = serde_json::from_str::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v["basis"].as_str().map(String::from))
                .unwrap_or_else(old_basis);
            let old = basis == BASIS_LIVE_AFTER_BUY;
            (r, old)
        })
        .collect()
}

/// Момент решения о входе: первое событие сделки «План…» (executor.rs), иначе начало сделки. Ошибок нет.
pub fn decided_at(d: &Deal) -> DateTime<Utc> {
    d.timeline
        .iter()
        .find(|(_, t)| t.starts_with("План"))
        .map(|(t, _)| *t)
        .unwrap_or(d.started_at)
}

/// Что делает: пересчёт одной сделки (см. шапку). Принимает: окружение, последнюю запись сделки, сделку из учёта
/// (None — нет), окна базы. Возвращает: Item. Ошибок нет — всё, что не так, становится причиной в строке отчёта.
pub async fn one(
    env: &Env<'_>,
    row: &DealRow,
    deal: Option<&Deal>,
    windows: &[WindowInfo],
) -> Item {
    let mut it = one_skip(row, deal);
    let res = match (Old::parse(&row.body), deal) {
        (Err(e), _) => Err(e),
        (_, None) => Err("сделки нет в учёте (core_state)".to_string()),
        (Ok(o), Some(d)) => run(env, row, d, o, windows).await,
    };
    match res {
        Ok(((group, missed, cap, source), r)) => {
            let l = &mut it.line;
            (l.group, l.now_missed, l.now_cap, l.source) = (group, Some(missed), Some(cap), source);
            it.row = Some(r);
        }
        Err(e) => it.line.reason = Some(e),
    }
    it
}

/// Итог пересчёта для строки отчёта: группа, упущено, потолок, источник.
type Done = (&'static str, Decimal, Decimal, String);

/// Подготовка (окно, стаканы, контекст, источник) и сам расчёт. Ошибки: текст причины «не пересчитано».
async fn run(
    env: &Env<'_>,
    row: &DealRow,
    d: &Deal,
    old: Old,
    windows: &[WindowInfo],
) -> Result<(Done, Row), String> {
    let t0 = Instant::now();
    let p: Prepared = prepare(env, d, &old, windows)?;
    let mut curve = super::size_curve::SourceCurve {
        cx: &p.cx,
        src: &p.src,
    };
    let (min_usd, min_pct) = (p.cfg.min_usd, p.cfg.min_pct);
    let u = super::size_uncapped::uncapped(&mut curve, &p.params, p.budget, &p.cfg.ladder, |pt| {
        super::card_max::passes(&pt.data, min_usd, min_pct)
    })
    .await
    .map_err(|e| format!("расчёт невозможен: {e}"))?;
    let inputs = Inputs {
        battle: old.battle,
        parts: old.parts,
        limits_error: old.limits_error,
        budget: p.budget,
        min_usd,
        min_pct,
        search_cap: p.cfg.search_cap,
        ms: t0.elapsed().as_secs_f64() * 1000.0,
        basis: p.basis.to_string(),
        decided_at: Some(p.decided_at),
        snapshot: Some(p.source.clone()),
    };
    let rec = record(inputs, u, Utc::now());
    let body = serde_json::to_string(&rec).map_err(|e| format!("запись не собирается: {e}"))?;
    let done = (p.group, rec.missed, rec.opt.invested, p.source);
    let r = Row {
        kind: "deal",
        deal_id: Some(d.id.clone()),
        ticker: d.direction.asset.ticker.clone(),
        address: d.direction.asset.bsc_contract.map(|a| format!("{a:#x}")),
        started_at: row.started_at.clone(),
        body,
    };
    Ok((done, r))
}

/// Направление текстом: «MCRT bybit → bsc_dex». Ошибок нет.
pub(super) fn dir_text(d: &Deal) -> String {
    let x = &d.direction;
    format!("{} {} → {}", x.asset.ticker, x.buy_on.0, x.sell_on.0)
}
