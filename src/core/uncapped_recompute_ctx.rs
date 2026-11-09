//! Файл: core/uncapped_recompute_ctx.rs
//! Что делает: решение 218 — подготовка пересчёта одной старой записи «без потолка» по окну сделки (uncapped_recompute.rs):
//! старая запись (бой, бюджет), план сделки, окно и стаканы бирж на момент решения (window_at.rs), настройки модели,
//! контекст направления DirCtx и источник данных Replay (uncapped_replay.rs), параметры поиска (size_curve.rs
//! search_params). Всё, чего не хватает, — причина «не пересчитано» (текст).
//! Контекст направления собирается из плана и настроек, а не из живого справочника (его на момент сделки нет): σ и
//! время в пути — из плана; k (profit.price_move_k), h (profit.liquidity_share_h), котируемые монеты
//! (venue.quote_asset), решение 240 — доля закрытия разницы (profit.closure_share [B]) — из настроек базы, как k; возврат
//! USDT в каждой сделке — был ли он в плане. Правил пары и данных сети
//! (минимумы вывода и депозита, точность вывода) на момент сделки в базе нет — в контексте их нет, проверки не идут
//! (пометка в записи). Шаг поиска — 10^-n, n — наибольшее число знаков у количества плана и количеств уровней стакана A
//! (количества кратны шагу биржи, поиск не грубее него).
//! Кто вызывает: core/uncapped_recompute.rs.
//! От чего зависит: uncapped_recompute_cfg.rs (старая запись, настройки, шаг), uncapped_replay.rs, window_at.rs,
//! dirctx.rs, size_curve.rs, storage/episode_files.rs, settings.
//! Настройки: card.max_search_usdt, money.min_profit_usd, money.min_profit_pct, uncapped.max_evals,
//! uncapped.ladder_usdt, size.*, venue.quote_asset [A, B], profit.price_move_k [монета], profit.liquidity_share_h [B],
//! profit.closure_share [B] (решение 240, core/reserve.rs). Решение 242: пороги прибыли, k и доля — сначала площадка + тип
//! направления сделки (core/venue_dir.rs rule по видам сторон из плана).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At as SAt;

use super::deal::Deal;
use super::dirctx::DirCtx;
use super::size::Params;
use super::size_curve::{cumulative, search_params};
use super::uncapped_recompute::{Env, G_CEX, G_DEX, decided_at};
pub use super::uncapped_recompute_cfg::{Cfg, Old};
use super::uncapped_recompute_cfg::{presence, step_of, venue_at};
use super::uncapped_replay::{Plan, Replay};
use super::uncapped_types::{BASIS_RECOMPUTED, BASIS_RECOMPUTED_DEX_PLAN};
use super::window::WindowInfo;
use super::window_at::books_before;

/// Готовый к расчёту пересчёт.
pub struct Prepared {
    pub cx: DirCtx,
    pub src: Replay,
    pub params: Params,
    pub budget: Decimal,
    pub cfg: Cfg,
    pub basis: &'static str,
    pub group: &'static str,
    pub decided_at: DateTime<Utc>,
    /// Текст источника для записи и отчёта.
    pub source: String,
}

/// Что делает: подготовка (см. шапку). Принимает: окружение, сделку, старую запись, окна. Возвращает: Prepared.
/// Ошибки: текст причины «не пересчитано».
pub fn prepare(
    env: &Env<'_>,
    d: &Deal,
    old: &Old,
    windows: &[WindowInfo],
) -> Result<Prepared, String> {
    let dir = &d.direction;
    if dir.buy_on == dir.sell_on {
        return Err("цикл DEX ↔ DEX — «без потолка» не считается".into());
    }
    let plan = Plan::parse(&d.plan)?;
    if plan.buy_quote.route.is_some() {
        return Err("DEX → биржа — не пересчитывается (решение 218)".into());
    }
    let dex = plan.sell_quote.route.is_some();
    let t = decided_at(d);
    let w = windows
        .iter()
        .find(|w| w.deals.iter().any(|x| x.id == d.id))
        .ok_or("окна у сделки нет")?;
    if w.started_at >= t {
        return Err(format!("окно {} открыто не раньше момента решения", w.id));
    }
    let mut want = vec![dir.buy_on.clone()];
    if !dex {
        want.push(dir.sell_on.clone());
    }
    for v in &want {
        if !w.venues.iter().any(|x| &x.venue == v) {
            return Err(format!("биржа {} в окне {} не писалась", v.0, w.id));
        }
    }
    let path = env.windows_dir.join(format!("{}.jsonl.zst", w.id));
    let (rows, _) = crate::storage::episode_files::read(&path, usize::MAX)
        .map_err(|e| format!("файл окна {}: {e}", path.display()))?;
    let t_ns = t
        .timestamp_nanos_opt()
        .ok_or("момент решения вне диапазона")?;
    let mut books = books_before(&rows, &want, &dir.asset, t_ns, env.decode);
    let mut take = |v: &VenueId| {
        books
            .remove(v)
            .unwrap_or_else(|| Err(format!("{}: нет в окне", v.0)))
    };
    let a = take(&dir.buy_on)?;
    if a.book.asks.is_empty() {
        return Err(format!("{}: в стакане окна нет продавцов", dir.buy_on.0));
    }
    let b = match dex {
        true => None,
        false => Some(take(&dir.sell_on)?),
    };
    if b.as_ref().is_some_and(|b| b.book.bids.is_empty()) {
        return Err(format!("{}: в стакане окна нет покупателей", dir.sell_on.0));
    }
    let s = env.settings;
    // Решение 242: площадка и тип направления сделки (правило стороны, core/venue_dir.rs). Ядра в пересчёте нет — вид
    // сторон из плана, как выше: покупка на DEX сюда не доходит (не пересчитывается), продажа на DEX — маршрут плана.
    let sell_kind = if dex { VenueKind::Dex } else { VenueKind::Cex };
    let vd = super::venue_dir::rule(dir, Some(VenueKind::Cex), Some(sell_kind));
    let cfg = Cfg::read(s, vd)?;
    let text = |n: &str, v: &VenueId| s.text(n, venue_at(v)).map_err(|e| e.message);
    let (qa, qb) = (
        text("venue.quote_asset", &dir.buy_on)?,
        text("venue.quote_asset", &dir.sell_on)?,
    );
    let coin = crate::settings::scope::coin_id(&dir.asset.ticker, dir.asset.bsc_contract.as_ref());
    let coin_at = SAt {
        venue: None,
        coin: Some(&coin),
    };
    let k = s
        .decimal_dir("profit.price_move_k", vd, coin_at)
        .map_err(|e| e.message)?;
    let h = s
        .decimal("profit.liquidity_share_h", venue_at(&dir.sell_on))
        .map_err(|e| e.message)?;
    if h.is_zero() {
        return Err("profit.liquidity_share_h = 0: глубину B использовать нельзя".into());
    }
    let closure_share = super::reserve::closure_share(s, dir, vd)?;
    let budget = old
        .budget
        .or(old.battle.budget)
        .or(old.parts.map(|p| p.cap()))
        .filter(|b| *b > Decimal::ZERO)
        .ok_or("в старой записи нет бюджета «на нашем потолке»")?;
    let src = Replay::new(
        dir,
        &plan,
        a.book.clone(),
        b.as_ref().map(|b| b.book.clone()),
        [&qa, &qb],
    )?;
    let step = step_of(&plan, &a.book);
    let sell_breaks = b
        .as_ref()
        .map(|b| cumulative(&b.book.bids))
        .unwrap_or_default();
    let price = a.book.asks[0].price;
    let mut params = search_params(
        s,
        step,
        cfg.search_cap,
        price,
        cumulative(&a.book.asks),
        sell_breaks,
    )?;
    params.max_evals = cfg.max_evals;
    let b_text = match &b {
        Some(b) => b.describe(),
        None => format!(
            "DEX-сторона из плана — приблизительно: средняя цена продажи плана {} как постоянная, без кривой пулов",
            plan.sell_quote.avg_price
        ),
    };
    let source = format!(
        "окно {}, момент решения {}: A — {}; B — {}. Комиссии, вывод, σ, время в пути, курсы затрат — из плана сделки; \
         правил пары и минимумов вывода и депозита на момент сделки в базе нет — не применялись; шаг поиска {step}",
        w.id,
        t.format("%Y-%m-%d %H:%M:%S%.3f"),
        a.describe(),
        b_text
    );
    let cx = DirCtx {
        dir: dir.clone(),
        a: presence(&dir.buy_on, &dir.asset),
        b: presence(&dir.sell_on, &dir.asset),
        net_a: String::new(),
        net_b: String::new(),
        quote: vec![qa.clone(), qb],
        usdt: Asset {
            ticker: qa,
            bsc_contract: None,
        },
        cex: vec![],
        sigma: plan.sigma,
        transit_s: plan.transit_s,
        k,
        h,
        closure_share,
        return_per_trade: plan.returns(),
        pair_a: None,
        pair_b: None,
        usdt_b: None,
        transfer_tax: None,
    };
    let (basis, group) = if dex {
        (BASIS_RECOMPUTED_DEX_PLAN, G_DEX)
    } else {
        (BASIS_RECOMPUTED, G_CEX)
    };
    Ok(Prepared {
        cx,
        src,
        params,
        budget,
        cfg,
        basis,
        group,
        decided_at: t,
        source,
    })
}
